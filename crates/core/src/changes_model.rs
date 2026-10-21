//! Port of changesModel.ts.
//!
//! The Git tab's Changes view as pure data: VS Code-style groups built from porcelain-v2 status plus numstat, the
//! rows and selection the pane shows, and the diff preview's line classification and first changed line. Git I/O
//! lives in `deckhand_sys::changes_git`; the records it produces are `deckhand_protocol` types.
//!
//! [`StatusEntry`] (workspaceGit.ts) is defined here because grouping consumes it; `deckhand_sys::workspace_git`
//! produces it.

use std::cmp::Ordering;
use std::sync::LazyLock;

use indexmap::IndexMap;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;

use deckhand_protocol::data::{ChangeCounts, ChangeEntry, ChangeGroup, ChangesRecord, JsNumber, StageMode};

use crate::jsstr::utf16_to_byte;

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
    CHANGE_GROUP_TITLES
        .iter()
        .find(|(candidate, _)| *candidate == group)
        .map_or("", |(_, title)| title)
}

/// TS: changesModel.ts MAX_CHANGES.
pub const MAX_CHANGES: usize = 2000;

/// TS: changesModel.ts LineCounts. JS numbers: `parseNumstat` yields NaN for a `-` deletions field and floats for
/// digit runs past 2^53, and `lineCountsText` prints whatever it is given.
#[skip_serializing_none]
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LineCounts {
    pub additions: Option<JsNumber>,
    pub deletions: Option<JsNumber>,
    pub binary: Option<bool>,
}

impl LineCounts {
    /// An entry's line counts (TS passes the entry itself as its LineCounts).
    pub fn of(entry: &ChangeEntry) -> LineCounts {
        #[allow(clippy::cast_precision_loss)]
        let number = |value: Option<i64>| value.map(|count| JsNumber(count as f64));
        LineCounts {
            additions: number(entry.additions),
            deletions: number(entry.deletions),
            binary: entry.binary,
        }
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
    ChangeCounts {
        conflicts: 0,
        staged: 0,
        unstaged: 0,
        untracked: 0,
    }
}

/// TS: changesModel.ts emptyChanges (callers set the fields TS passes in afterwards).
pub fn empty_changes() -> ChangesRecord {
    ChangesRecord {
        loaded: false,
        entries: Vec::new(),
        counts: empty_counts(),
        omitted: empty_counts(),
        branch: None,
        error: None,
        session_id: None,
        workspace: None,
    }
}

fn count_of(counts: &ChangeCounts, group: ChangeGroup) -> i64 {
    match group {
        ChangeGroup::Conflicts => counts.conflicts,
        ChangeGroup::Staged => counts.staged,
        ChangeGroup::Unstaged => counts.unstaged,
        ChangeGroup::Untracked => counts.untracked,
    }
}

fn count_mut(counts: &mut ChangeCounts, group: ChangeGroup) -> &mut i64 {
    match group {
        ChangeGroup::Conflicts => &mut counts.conflicts,
        ChangeGroup::Staged => &mut counts.staged,
        ChangeGroup::Unstaged => &mut counts.unstaged,
        ChangeGroup::Untracked => &mut counts.untracked,
    }
}

fn group_order(group: ChangeGroup) -> usize {
    CHANGE_GROUPS
        .iter()
        .position(|candidate| *candidate == group)
        .unwrap_or(CHANGE_GROUPS.len())
}

/// `Number(text)` for a `\d+|-` numstat field: `-` is NaN.
fn js_number_of(text: &str) -> JsNumber {
    JsNumber(text.parse::<f64>().unwrap_or(f64::NAN))
}

/// A numstat count field: `\d+` (ASCII digits, as JS `\d`) or `-`.
fn is_count_field(text: &str) -> bool {
    text == "-" || (!text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit()))
}

/// TS: changesModel.ts parseNumstat (`git diff --numstat -z` by new path; the last record of a path wins).
pub fn parse_numstat(raw: &str) -> IndexMap<String, LineCounts> {
    let mut counts = IndexMap::new();
    let records: Vec<&str> = raw.split('\0').collect();
    let mut index = 0;
    while let Some(record) = records.get(index) {
        // `^(\d+|-)\t(\d+|-)\t([\s\S]*)$`: neither count field can hold a tab, so the first two tabs delimit them.
        let fields = record
            .split_once('\t')
            .and_then(|(added, rest)| rest.split_once('\t').map(|(deleted, file)| (added, deleted, file)))
            .filter(|(added, deleted, _)| is_count_field(added) && is_count_field(deleted));
        if let Some((added, deleted, file)) = fields {
            let mut file = file.to_owned();
            // A rename: empty path, then the old and new paths.
            if file.is_empty() {
                file = records.get(index + 2).copied().unwrap_or("").to_owned();
                index += 2;
            }
            let value = if added == "-" {
                LineCounts {
                    binary: Some(true),
                    ..LineCounts::default()
                }
            } else {
                LineCounts {
                    additions: Some(js_number_of(added)),
                    deletions: Some(js_number_of(deleted)),
                    binary: None,
                }
            };
            // IndexMap keeps a re-inserted key's first position, as Map.set does.
            counts.insert(file, value);
        }
        index += 1;
    }
    counts
}

/// TS: changesModel.ts groupChanges `numstat` argument (per group's diff).
#[derive(Clone, Debug, Default, PartialEq)]
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

/// JS `<` on strings: UTF-16 code unit order (differs from UTF-8 byte order above U+FFFF).
fn js_string_cmp(left: &str, right: &str) -> Ordering {
    left.encode_utf16().cmp(right.encode_utf16())
}

/// `{...entry, ...counts}`: the numstat's line counts on a new entry. The wire type holds integers; a NaN or fractional
/// count (never produced by git) truncates the way `as` does.
#[allow(clippy::cast_possible_truncation)]
fn with_counts(mut entry: ChangeEntry, counts: Option<&LineCounts>) -> ChangeEntry {
    if let Some(counts) = counts {
        if let Some(additions) = counts.additions {
            entry.additions = Some(additions.0 as i64);
        }
        if let Some(deletions) = counts.deletions {
            entry.deletions = Some(deletions.0 as i64);
        }
        if let Some(binary) = counts.binary {
            entry.binary = Some(binary);
        }
    }
    entry
}

fn new_entry(group: ChangeGroup, path: &str, status: &str) -> ChangeEntry {
    ChangeEntry {
        group,
        path: path.to_owned(),
        orig_path: None,
        status: status.to_owned(),
        conflict: None,
        additions: None,
        deletions: None,
        binary: None,
    }
}

/// TS: changesModel.ts groupChanges (`cap` defaults to MAX_CHANGES).
///
/// VS Code's groups from porcelain-v2 entries: unmerged → Merge Conflicts; the index side (X) → Staged Changes; the
/// worktree side (Y) → Changes; untracked → Untracked. A partially staged file is in both Staged and Changes.
pub fn group_changes(entries: &[StatusEntry], numstat: &Numstats, cap: usize) -> GroupedChanges {
    let lookup = |map: &Option<IndexMap<String, LineCounts>>, path: &str| -> Option<LineCounts> {
        map.as_ref().and_then(|map| map.get(path)).copied()
    };
    let mut groups: [Vec<ChangeEntry>; 4] = Default::default();
    for entry in entries {
        let path = entry.path.as_str();
        match entry.kind {
            StatusKind::Untracked => {
                let change = new_entry(ChangeGroup::Untracked, path, "?");
                groups[3].push(with_counts(change, lookup(&numstat.untracked, path).as_ref()));
            }
            StatusKind::Unmerged => {
                let mut change = new_entry(ChangeGroup::Conflicts, path, "U");
                change.conflict = Some(entry.xy.clone());
                groups[0].push(with_counts(change, lookup(&numstat.unstaged, path).as_ref()));
            }
            StatusKind::Ordinary | StatusKind::Renamed => {
                // `const [index = '.', worktree = '.'] = entry.xy` (string destructuring is per code point).
                let mut sides = entry.xy.chars();
                let index = sides.next().unwrap_or('.');
                let worktree = sides.next().unwrap_or('.');
                if index != '.' {
                    let mut change = new_entry(ChangeGroup::Staged, path, index.encode_utf8(&mut [0; 4]));
                    if matches!(index, 'R' | 'C') {
                        change.orig_path.clone_from(&entry.orig_path);
                    }
                    groups[1].push(with_counts(change, lookup(&numstat.staged, path).as_ref()));
                }
                if worktree != '.' {
                    let change = new_entry(ChangeGroup::Unstaged, path, worktree.encode_utf8(&mut [0; 4]));
                    groups[2].push(with_counts(change, lookup(&numstat.unstaged, path).as_ref()));
                }
            }
        }
    }
    let mut counts = empty_counts();
    let mut omitted = empty_counts();
    let mut shown = Vec::new();
    let mut room = cap;
    for (group, mut list) in CHANGE_GROUPS.into_iter().zip(groups) {
        list.sort_by(|left, right| js_string_cmp(&left.path, &right.path));
        let length = list.len();
        let take = room.min(length);
        *count_mut(&mut counts, group) = i64::try_from(length).unwrap_or(i64::MAX);
        *count_mut(&mut omitted, group) = i64::try_from(length - take).unwrap_or(i64::MAX);
        shown.extend(list.into_iter().take(take));
        room -= take;
    }
    GroupedChanges {
        entries: shown,
        counts,
        omitted,
    }
}

/// TS: changesModel.ts changeKey (`<group>\0<path>`).
pub fn change_key(entry: &ChangeEntry) -> String {
    format!("{}\0{}", entry.group.as_str(), entry.path)
}

/// TS: changesModel.ts findChange.
pub fn find_change<'a>(entries: &'a [ChangeEntry], group: ChangeGroup, file: &str) -> Option<&'a ChangeEntry> {
    entries.iter().find(|entry| entry.group == group && entry.path == file)
}

/// TS: changesModel.ts totalChanges.
pub fn total_changes(counts: &ChangeCounts) -> i64 {
    CHANGE_GROUPS
        .iter()
        .fold(0i64, |sum, group| sum.saturating_add(count_of(counts, *group)))
}

/// TS: changesModel.ts ChangeRow.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
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

/// TS: changesModel.ts changeRows (a header per non-empty group, its entries, and "+N more" where the cap cut it).
pub fn change_rows(record: &ChangesRecord) -> Vec<ChangeRow> {
    let mut rows = Vec::new();
    let mut index = 0;
    for group in CHANGE_GROUPS {
        let count = count_of(&record.counts, group);
        if count == 0 {
            continue;
        }
        rows.push(ChangeRow::Header { group, count });
        while let Some(entry) = record.entries.get(index).filter(|entry| entry.group == group) {
            rows.push(ChangeRow::Entry {
                entry: entry.clone(),
                index,
            });
            index += 1;
        }
        let more = count_of(&record.omitted, group);
        if more != 0 {
            rows.push(ChangeRow::More { group, count: more });
        }
    }
    rows
}

/// TS: changesModel.ts reselect `previous` argument.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PreviousSelection {
    pub key: Option<String>,
    pub index: usize,
    pub offset: Option<usize>,
}

/// TS: changesModel.ts reselect (TS -1, nothing to select: `None`).
///
/// The same (group, path) when it is still listed; else the entry now at the same position within that group (so
/// staging a file selects the next one in its group), or its last entry; when the group emptied, the first entry
/// below it (else the last one).
pub fn reselect(entries: &[ChangeEntry], previous: &PreviousSelection) -> Option<usize> {
    let last = entries.len().checked_sub(1)?;
    if let Some(key) = &previous.key
        && let Some(same) = entries.iter().position(|entry| change_key(entry) == *key)
    {
        return Some(same);
    }
    let group = previous.key.as_deref().map(|key| key.split('\0').next().unwrap_or(key));
    let in_group = |entry: &ChangeEntry| group == Some(entry.group.as_str());
    if let Some(start) = entries.iter().position(in_group) {
        let mut end = start;
        while entries.get(end + 1).is_some_and(in_group) {
            end += 1;
        }
        return Some(end.min(start.saturating_add(previous.offset.unwrap_or(0))));
    }
    // The group emptied: the first entry of the next non-empty group below it, else the last one above it.
    if let Some(order) = group.and_then(|group| CHANGE_GROUPS.iter().position(|candidate| candidate.as_str() == group))
    {
        let below = entries.iter().position(|entry| group_order(entry.group) > order);
        return Some(below.unwrap_or(last));
    }
    Some(last.min(previous.index))
}

/// TS: changesModel.ts groupOffset (an entry's position within its group).
pub fn group_offset(entries: &[ChangeEntry], index: usize) -> usize {
    // TS reads `entries[index]?.group`: past the end it is undefined, which no entry's group equals.
    let Some(group) = entries.get(index).map(|entry| entry.group) else {
        return 0;
    };
    let mut offset = 0;
    while index > offset
        && entries
            .get(index - offset - 1)
            .is_some_and(|entry| entry.group == group)
    {
        offset += 1;
    }
    offset
}

/// TS: changesModel.ts stageMode (unstaged, untracked and conflicted entries are staged; staged ones unstaged).
pub fn stage_mode(entry: &ChangeEntry) -> StageMode {
    if entry.group == ChangeGroup::Staged {
        StageMode::Unstage
    } else {
        StageMode::Stage
    }
}

/// TS: changesModel.ts changeLabel result.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangeLabel {
    pub name: String,
    pub dir: String,
}

fn split_label(file: &str) -> ChangeLabel {
    let trimmed = file.strip_suffix('/').unwrap_or(file);
    let cut = trimmed.rfind('/');
    let name = &trimmed[cut.map_or(0, |cut| cut + 1)..];
    ChangeLabel {
        name: format!("{name}{}", if file.ends_with('/') { "/" } else { "" }),
        dir: cut.map_or("", |cut| &trimmed[..cut]).to_owned(),
    }
}

/// TS: changesModel.ts changeLabel (VS Code style; a rename reads `old → new`, names only when in one directory).
pub fn change_label(entry: &ChangeEntry) -> ChangeLabel {
    let target = split_label(&entry.path);
    let Some(orig_path) = &entry.orig_path else {
        return target;
    };
    let source = split_label(orig_path);
    if source.dir == target.dir {
        ChangeLabel {
            name: format!("{} → {}", source.name, target.name),
            dir: target.dir,
        }
    } else {
        ChangeLabel {
            name: format!("{orig_path} → {}", entry.path),
            dir: String::new(),
        }
    }
}

/// `${number}` (Number.prototype.toString).
fn js_number_text(number: JsNumber) -> String {
    let value = number.0;
    if value.is_nan() {
        "NaN".to_owned()
    } else if value.is_infinite() {
        if value > 0.0 { "Infinity" } else { "-Infinity" }.to_owned()
    } else {
        number.to_js_string()
    }
}

/// TS: changesModel.ts lineCountsText (`+12 −3`, `bin`, or empty).
pub fn line_counts_text(counts: &LineCounts) -> String {
    if counts.binary == Some(true) {
        return "bin".to_owned();
    }
    match (counts.additions, counts.deletions) {
        (Some(additions), Some(deletions)) => {
            format!("+{} −{}", js_number_text(additions), js_number_text(deletions))
        }
        _ => String::new(),
    }
}

/// `^(@{2,}) (?:-\d+(?:,\d+)? )+\+(\d+)(?:,(\d+))? @{2,}` (JS `\d` is ASCII).
static HUNK_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(@{2,}) (?:-[0-9]+(?:,[0-9]+)? )+\+([0-9]+)(?:,([0-9]+))? @{2,}")
        .expect("HUNK_PATTERN is a valid regex")
});

/// `line.slice(0, parents)`: the prefix columns (UTF-16 units; a surrogate pair cut in half holds no `+`/`-`).
fn prefix_columns(line: &str, parents: usize) -> &str {
    &line[..utf16_to_byte(line, parents)]
}

/// TS: changesModel.ts firstChangedLine (on the new side; 1 without a hunk).
///
/// Walks the first hunk's lines, counting new-side lines until the first added or removed one.
pub fn first_changed_line(diff: &str) -> i64 {
    let lines: Vec<&str> = diff.split('\n').collect();
    let Some((start, captures)) = lines
        .iter()
        .enumerate()
        .find_map(|(index, line)| HUNK_PATTERN.captures(line).map(|captures| (index, captures)))
    else {
        return 1;
    };
    let parents = captures.get(1).map_or(0, |ats| ats.as_str().len() - 1);
    let mut line = captures
        .get(2)
        .map_or(f64::NAN, |from| from.as_str().parse::<f64>().unwrap_or(f64::NAN));
    for text in &lines[start + 1..] {
        if HUNK_PATTERN.is_match(text) || text.starts_with("diff ") {
            break;
        }
        if text.starts_with('\\') {
            continue;
        }
        if prefix_columns(text, parents).contains(['+', '-']) {
            break;
        }
        line += 1.0;
    }
    #[allow(clippy::cast_possible_truncation)]
    let first = line.max(1.0) as i64;
    first
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
///
/// Header lines (`diff`, `index`, `---`/`+++`, modes, renames) count only before a file's first hunk; a combined
/// diff's (conflict) prefixes span one column per parent.
pub fn classify_diff(text: &str) -> Vec<DiffLine> {
    let mut parents = 1;
    let mut header = true;
    text.strip_suffix('\n')
        .unwrap_or(text)
        .split('\n')
        .map(|line| {
            let kind = if let Some(ats) = HUNK_PATTERN.captures(line).and_then(|captures| captures.get(1)) {
                parents = ats.as_str().len() - 1;
                header = false;
                DiffLineKind::Hunk
            } else if line.starts_with("diff ") {
                header = true;
                DiffLineKind::Meta
            } else if header {
                DiffLineKind::Meta
            } else {
                let prefix = prefix_columns(line, parents);
                if prefix.contains('-') {
                    DiffLineKind::Del
                } else if prefix.contains('+') {
                    DiffLineKind::Add
                } else {
                    DiffLineKind::Context
                }
            };
            DiffLine {
                kind,
                text: line.to_owned(),
            }
        })
        .collect()
}

/// TS: changesModel.ts untrackedDiff (a file's contents as an all-additions diff, so it renders and opens like any
/// other entry).
pub fn untracked_diff(content: &str) -> String {
    let body = content.strip_suffix('\n').unwrap_or(content);
    let lines: Vec<&str> = if body.is_empty() {
        Vec::new()
    } else {
        body.split('\n').collect()
    };
    let mut out = format!("@@ -0,0 +1,{} @@", lines.len());
    for line in lines {
        out.push_str("\n+");
        out.push_str(line);
    }
    out
}

#[cfg(test)]
mod tests {
    use deckhand_testkit::{Case, Outcome, replay, tags};
    use serde_json::{Value, json};

    use super::*;

    fn status(kind: StatusKind, xy: &str, path: &str) -> StatusEntry {
        StatusEntry {
            kind,
            xy: xy.to_owned(),
            path: path.to_owned(),
            orig_path: None,
        }
    }

    /// `parseStatus(...).entries` for porcelain-v2 ordinary records (`1 <xy> ... <path>`), untracked (`? <path>`).
    fn ordinary(records: &[(&str, &str)]) -> Vec<StatusEntry> {
        records
            .iter()
            .map(|(xy, path)| status(StatusKind::Ordinary, xy, path))
            .collect()
    }

    fn record(grouped: &GroupedChanges) -> ChangesRecord {
        ChangesRecord {
            entries: grouped.entries.clone(),
            counts: grouped.counts.clone(),
            omitted: grouped.omitted.clone(),
            ..empty_changes()
        }
    }

    fn counts(conflicts: i64, staged: i64, unstaged: i64, untracked: i64) -> ChangeCounts {
        ChangeCounts {
            conflicts,
            staged,
            unstaged,
            untracked,
        }
    }

    fn label_of(path: &str, orig_path: Option<&str>) -> ChangeLabel {
        let mut entry = new_entry(ChangeGroup::Unstaged, path, "M");
        entry.orig_path = orig_path.map(str::to_owned);
        change_label(&entry)
    }

    fn label(name: &str, dir: &str) -> ChangeLabel {
        ChangeLabel {
            name: name.to_owned(),
            dir: dir.to_owned(),
        }
    }

    // covers: B-P7-01 Changes groups follow VS Code: conflicts, staged, unstaged, untracked; partially staged files are in both
    #[test]
    fn groups_follow_vs_code() {
        // Porcelain v2 -z: an ordinary change (partially staged), a staged rename with spaces, a staged deletion, an
        // intent-to-add file, a conflict and untracked files (as parseStatus returns them).
        let entries = vec![
            status(StatusKind::Ordinary, "MM", "src/app.ts"),
            StatusEntry {
                orig_path: Some("docs/old name.md".to_owned()),
                ..status(StatusKind::Renamed, "R.", "docs/new name.md")
            },
            status(StatusKind::Ordinary, "D.", "gone.txt"),
            status(StatusKind::Ordinary, ".A", "intent.txt"),
            status(StatusKind::Unmerged, "UU", "both.txt"),
            status(StatusKind::Untracked, "??", "z-last.txt"),
            status(StatusKind::Untracked, "??", "a first.bin"),
        ];
        let staged = parse_numstat(
            &[
                "3\t1\tsrc/app.ts",
                "0\t0\t",
                "docs/old name.md",
                "docs/new name.md",
                "0\t4\tgone.txt",
                "",
            ]
            .join("\0"),
        );
        let unstaged = parse_numstat(
            &[
                "2\t0\tsrc/app.ts",
                "0\t0\tboth.txt",
                "5\t2\tboth.txt",
                "0\t0\tintent.txt",
                "",
            ]
            .join("\0"),
        );
        let untracked = IndexMap::from([
            (
                "a first.bin".to_owned(),
                LineCounts {
                    binary: Some(true),
                    ..LineCounts::default()
                },
            ),
            (
                "z-last.txt".to_owned(),
                LineCounts {
                    additions: Some(JsNumber(7.0)),
                    deletions: Some(JsNumber(0.0)),
                    binary: None,
                },
            ),
        ]);
        let numstat = Numstats {
            staged: Some(staged),
            unstaged: Some(unstaged),
            untracked: Some(untracked),
        };
        let GroupedChanges {
            entries,
            counts: totals,
            omitted,
        } = group_changes(&entries, &numstat, MAX_CHANGES);
        assert_eq!(totals, counts(1, 3, 2, 2));
        assert_eq!(omitted, counts(0, 0, 0, 0));
        let summary: Vec<(&str, &str, &str)> = entries
            .iter()
            .map(|entry| (entry.group.as_str(), entry.status.as_str(), entry.path.as_str()))
            .collect();
        assert_eq!(
            summary,
            [
                ("conflicts", "U", "both.txt"),
                ("staged", "R", "docs/new name.md"),
                ("staged", "D", "gone.txt"),
                ("staged", "M", "src/app.ts"),
                ("unstaged", "A", "intent.txt"),
                ("unstaged", "M", "src/app.ts"),
                ("untracked", "?", "a first.bin"),
                ("untracked", "?", "z-last.txt"),
            ]
        );
        let [conflict, rename, _, staged_app, _, unstaged_app, binary, text] = entries.as_slice() else {
            panic!("eight entries");
        };
        assert_eq!(conflict.conflict.as_deref(), Some("UU"));
        assert_eq!(conflict.additions, Some(5)); // A conflict's combined numstat: its last record.
        assert_eq!(rename.orig_path.as_deref(), Some("docs/old name.md"));
        assert_eq!(rename.additions, Some(0));
        assert_eq!(
            [
                staged_app.additions,
                staged_app.deletions,
                unstaged_app.additions,
                unstaged_app.deletions
            ],
            [Some(3), Some(1), Some(2), Some(0)]
        );
        assert_eq!(line_counts_text(&LineCounts::of(binary)), "bin");
        assert_eq!(line_counts_text(&LineCounts::of(text)), "+7 −0");
        assert_eq!(line_counts_text(&LineCounts::default()), "");
        assert_eq!(unstaged_app.orig_path, None);
        assert_eq!(stage_mode(staged_app), StageMode::Unstage);
        assert_eq!(stage_mode(unstaged_app), StageMode::Stage);
        assert_eq!(stage_mode(conflict), StageMode::Stage);
        assert_eq!(stage_mode(text), StageMode::Stage);
    }

    // covers: B-P7-02 the cap keeps group order, full counts and "+N more" rows
    #[test]
    fn cap_keeps_group_order() {
        let mut entries = ordinary(&[("M.", "s1"), ("M.", "s2"), (".M", "u1")]);
        entries.push(status(StatusKind::Untracked, "??", "n1"));
        entries.push(status(StatusKind::Untracked, "??", "n2"));
        let capped = group_changes(&entries, &Numstats::default(), 3);
        assert_eq!(capped.counts, counts(0, 2, 1, 2));
        assert_eq!(capped.omitted, counts(0, 0, 0, 2));
        let rows: Vec<String> = change_rows(&record(&capped))
            .iter()
            .map(|row| match row {
                ChangeRow::Entry { entry, .. } => entry.path.clone(),
                ChangeRow::Header { group, count } => format!("header:{group}:{count}"),
                ChangeRow::More { group, count } => format!("more:{group}:{count}"),
            })
            .collect();
        assert_eq!(
            rows,
            [
                "header:staged:2",
                "s1",
                "s2",
                "header:unstaged:1",
                "u1",
                "header:untracked:2",
                "more:untracked:2"
            ]
        );
        assert_eq!(
            change_rows(&record(&group_changes(&[], &Numstats::default(), MAX_CHANGES))),
            []
        );
    }

    // covers: B-P7-03 selection stays on the same group and path across refreshes, else the same position
    #[test]
    fn selection_stays_across_refreshes() {
        let select = |key: &str, index: usize, offset: Option<usize>| PreviousSelection {
            key: Some(key.to_owned()),
            index,
            offset,
        };
        let group =
            |records: &[(&str, &str)]| group_changes(&ordinary(records), &Numstats::default(), MAX_CHANGES).entries;
        let before = group(&[(".M", "a"), (".M", "b"), (".M", "c")]);
        assert_eq!(reselect(&before, &select(&change_key(&before[1]), 1, None)), Some(1));
        assert_eq!(group_offset(&before, 2), 2);
        // b was staged: it left Changes, so the selection moves to what is now at its position there (c), not to the
        // staged b that took its overall position.
        let after = group(&[("M.", "b"), (".M", "a"), (".M", "c")]);
        let path = |selection: PreviousSelection| reselect(&after, &selection).map(|index| after[index].path.clone());
        assert_eq!(path(select("unstaged\0b", 1, Some(1))).as_deref(), Some("c"));
        assert_eq!(path(select("unstaged\0c", 0, None)).as_deref(), Some("c"));
        // The last of a group goes to the new last one; an emptied group falls back to the overall position.
        assert_eq!(path(select("unstaged\0z", 5, Some(4))).as_deref(), Some("c"));
        // An emptied group: the first entry below it, else the last entry; an unknown key keeps the position.
        assert_eq!(reselect(&after, &select("conflicts\0gone", 2, None)), Some(0));
        assert_eq!(
            reselect(&after, &select("untracked\0gone", 0, None)),
            Some(after.len() - 1)
        );
        assert_eq!(reselect(&after, &select("other", 99, None)), Some(after.len() - 1));
        assert_eq!(group_offset(&after, 2), 1);
        assert_eq!(reselect(&[], &PreviousSelection::default()), None);
    }

    // covers: B-P7-04 labels put the name first; renames read old → new
    #[test]
    fn labels_put_the_name_first() {
        assert_eq!(label_of("src/ui/app.tsx", None), label("app.tsx", "src/ui"));
        assert_eq!(label_of("top level.md", None), label("top level.md", ""));
        assert_eq!(
            label_of("docs/new name.md", Some("docs/old name.md")),
            label("old name.md → new name.md", "docs")
        );
        assert_eq!(label_of("b/x.ts", Some("a/x.ts")), label("a/x.ts → b/x.ts", ""));
        assert_eq!(label_of("vendor/nested/", None), label("nested/", "vendor"));
    }

    // covers: B-P7-05 first changed line: the new-side line of the first hunk's first change
    #[test]
    fn first_changed_line_new_side() {
        let unified = [
            "diff --git a/f b/f",
            "index 1..2 100644",
            "--- a/f",
            "+++ b/f",
            "@@ -10,7 +12,8 @@ function x() {",
            " one",
            " two",
            " three",
            "-old",
            "+new",
            " four",
            "@@ -40 +41 @@",
            "-x",
            "+y",
        ]
        .join("\n");
        assert_eq!(first_changed_line(&unified), 15);
        assert_eq!(first_changed_line(&["@@ -0,0 +1,2 @@", "+a", "+b"].join("\n")), 1);
        assert_eq!(first_changed_line(&["@@ -1,3 +0,0 @@", "-a"].join("\n")), 1); // Everything deleted.
        assert_eq!(
            first_changed_line(&["@@ -3,2 +3 @@", " keep", "\\ No newline at end of file", "-gone"].join("\n")),
            4
        );
        // A conflict's combined diff: two prefix columns, the new side is the last range.
        assert_eq!(
            first_changed_line(
                &[
                    "diff --cc both.txt",
                    "@@@ -1,2 -1,2 +1,6 @@@",
                    "  x",
                    "++<<<<<<< HEAD",
                    " +main"
                ]
                .join("\n")
            ),
            2
        );
        assert_eq!(first_changed_line("Binary files a/x and b/x differ"), 1);
        assert_eq!(first_changed_line(""), 1);
    }

    // covers: B-P7-06 diff lines are classified for colour; header lines only before the first hunk
    #[test]
    fn diff_lines_are_classified() {
        use DiffLineKind::{Add, Context, Del, Hunk, Meta};
        let kinds = |text: &str| {
            classify_diff(text)
                .into_iter()
                .map(|line| line.kind)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            kinds(
                &[
                    "diff --git a/f b/f",
                    "new file mode 100644",
                    "--- /dev/null",
                    "+++ b/f",
                    "@@ -0,0 +1,3 @@",
                    "+a",
                    "--- not a header",
                    " ctx",
                    "\\ No newline at end of file",
                ]
                .join("\n")
            ),
            [Meta, Meta, Meta, Meta, Hunk, Add, Del, Context, Context]
        );
        assert_eq!(
            kinds(
                &[
                    "diff --cc both.txt",
                    "index 1,2..0",
                    "@@@ -1,2 -1,2 +1,6 @@@",
                    "  x",
                    "++<<<<<<< HEAD",
                    " +main",
                    "+ other",
                    " -gone",
                ]
                .join("\n")
            ),
            [Meta, Meta, Hunk, Context, Add, Add, Add, Del]
        );
        let untracked = untracked_diff("one\ntwo\n");
        assert_eq!(untracked, "@@ -0,0 +1,2 @@\n+one\n+two");
        assert_eq!(kinds(&untracked), [Hunk, Add, Add]);
        assert_eq!(untracked_diff(""), "@@ -0,0 +1,0 @@");
    }

    // ---- parity replays -----------------------------------------------------------------------------------------

    /// A recorded ChangeEntry-shaped argument that only some fields of matter (`Pick<ChangeEntry, ...>` in TS).
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct PartialEntry {
        group: Option<ChangeGroup>,
        #[serde(default)]
        path: String,
        orig_path: Option<String>,
    }

    impl PartialEntry {
        fn entry(self) -> ChangeEntry {
            let mut entry = new_entry(self.group.unwrap_or(ChangeGroup::Unstaged), &self.path, "");
            entry.orig_path = self.orig_path;
            entry
        }
    }

    /// An optional JS number argument clamped the way the TS uses it (`Math.max(0, n)`): negative → 0.
    fn clamp(value: Option<i64>) -> Option<usize> {
        value.map(|number| usize::try_from(number).unwrap_or(0))
    }

    #[derive(Deserialize)]
    struct RecordedSelection {
        key: Option<String>,
        index: i64,
        offset: Option<i64>,
    }

    /// workspaceGit.ts StatusEntry as recorded: any kind other than `?`/`u` takes groupChanges' ordinary path.
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct RecordedStatus {
        kind: String,
        xy: String,
        path: String,
        orig_path: Option<String>,
    }

    #[derive(Deserialize)]
    struct RecordedNumstats {
        staged: Option<IndexMap<String, LineCounts>>,
        unstaged: Option<IndexMap<String, LineCounts>>,
        untracked: Option<IndexMap<String, LineCounts>>,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct RecordedFields {
        loaded: Option<bool>,
        branch: Option<String>,
        error: Option<String>,
        session_id: Option<String>,
        workspace: Option<String>,
    }

    fn undefined_or<T: Serialize>(value: Option<T>) -> Result<Value, String> {
        value.map_or_else(
            || Ok(tags::undefined()),
            |value| serde_json::to_value(value).map_err(|e| e.to_string()),
        )
    }

    #[test]
    fn empty_counts_parity() {
        replay("changesModel/emptyCounts", |_: &Case| Outcome::ret(empty_counts()));
    }

    #[test]
    fn empty_changes_parity() {
        replay(
            "changesModel/emptyChanges",
            |case: &Case| -> Result<ChangesRecord, String> {
                let mut record = empty_changes();
                if !case.arg_is_undefined(0) {
                    let fields: RecordedFields = case.arg_as(0)?;
                    record.loaded = fields.loaded.unwrap_or(record.loaded);
                    record.branch = fields.branch;
                    record.error = fields.error;
                    record.session_id = fields.session_id;
                    record.workspace = fields.workspace;
                }
                Ok(record)
            },
        );
    }

    #[test]
    fn parse_numstat_parity() {
        replay("changesModel/parseNumstat", |case: &Case| -> Result<Value, String> {
            let parsed = parse_numstat(case.arg_str(0)?);
            let entries = parsed
                .into_iter()
                .map(|(file, counts)| Ok((json!(file), serde_json::to_value(counts).map_err(|e| e.to_string())?)))
                .collect::<Result<Vec<_>, String>>()?;
            Ok(tags::map(entries))
        });
    }

    #[test]
    fn group_changes_parity() {
        replay(
            "changesModel/groupChanges",
            |case: &Case| -> Result<GroupedChanges, String> {
                let recorded: Vec<RecordedStatus> = case.arg_as(0)?;
                let entries: Vec<StatusEntry> = recorded
                    .into_iter()
                    .map(|entry| StatusEntry {
                        kind: match entry.kind.as_str() {
                            "?" => StatusKind::Untracked,
                            "u" => StatusKind::Unmerged,
                            "2" => StatusKind::Renamed,
                            _ => StatusKind::Ordinary,
                        },
                        xy: entry.xy,
                        path: entry.path,
                        orig_path: entry.orig_path,
                    })
                    .collect();
                let numstat = if case.arg_is_undefined(1) {
                    Numstats::default()
                } else {
                    let recorded: RecordedNumstats = case.arg_as(1)?;
                    Numstats {
                        staged: recorded.staged,
                        unstaged: recorded.unstaged,
                        untracked: recorded.untracked,
                    }
                };
                let cap = if case.arg_is_undefined(2) {
                    Some(MAX_CHANGES)
                } else {
                    clamp(Some(case.arg_i64(2)?))
                };
                Ok(group_changes(&entries, &numstat, cap.unwrap_or(0)))
            },
        );
    }

    #[test]
    fn change_key_parity() {
        replay("changesModel/changeKey", |case: &Case| -> Result<String, String> {
            Ok(change_key(&case.arg_as::<ChangeEntry>(0)?))
        });
    }

    #[test]
    fn find_change_parity() {
        replay("changesModel/findChange", |case: &Case| -> Result<Value, String> {
            let entries: Vec<ChangeEntry> = case.arg_as(0)?;
            let group: ChangeGroup = case.arg_as(1)?;
            undefined_or(find_change(&entries, group, case.arg_str(2)?))
        });
    }

    #[test]
    fn total_changes_parity() {
        replay("changesModel/totalChanges", |case: &Case| -> Result<i64, String> {
            Ok(total_changes(&case.arg_as(0)?))
        });
    }

    #[test]
    fn change_rows_parity() {
        replay(
            "changesModel/changeRows",
            |case: &Case| -> Result<Vec<ChangeRow>, String> {
                Ok(change_rows(&record(&case.arg_as::<GroupedChanges>(0)?)))
            },
        );
    }

    #[test]
    fn reselect_parity() {
        replay("changesModel/reselect", |case: &Case| -> Result<i64, String> {
            let entries: Vec<ChangeEntry> = case.arg_as(0)?;
            let recorded: RecordedSelection = case.arg_as(1)?;
            let previous = PreviousSelection {
                key: recorded.key,
                index: clamp(Some(recorded.index)).unwrap_or(0),
                offset: clamp(recorded.offset),
            };
            Ok(reselect(&entries, &previous).map_or(-1, |index| i64::try_from(index).unwrap_or(i64::MAX)))
        });
    }

    #[test]
    fn group_offset_parity() {
        replay("changesModel/groupOffset", |case: &Case| -> Result<usize, String> {
            let entries: Vec<ChangeEntry> = case.arg_as(0)?;
            // A negative index reads `entries[-1]`: undefined, like any index past the end.
            let index = usize::try_from(case.arg_i64(1)?).unwrap_or(usize::MAX);
            Ok(group_offset(&entries, index))
        });
    }

    #[test]
    fn stage_mode_parity() {
        replay("changesModel/stageMode", |case: &Case| -> Result<StageMode, String> {
            Ok(stage_mode(&case.arg_as::<PartialEntry>(0)?.entry()))
        });
    }

    #[test]
    fn change_label_parity() {
        replay(
            "changesModel/changeLabel",
            |case: &Case| -> Result<ChangeLabel, String> { Ok(change_label(&case.arg_as::<PartialEntry>(0)?.entry())) },
        );
    }

    #[test]
    fn line_counts_text_parity() {
        replay("changesModel/lineCountsText", |case: &Case| -> Result<String, String> {
            Ok(line_counts_text(&case.arg_as(0)?))
        });
    }

    #[test]
    fn first_changed_line_parity() {
        replay("changesModel/firstChangedLine", |case: &Case| -> Result<i64, String> {
            Ok(first_changed_line(case.arg_str(0)?))
        });
    }

    #[test]
    fn classify_diff_parity() {
        replay(
            "changesModel/classifyDiff",
            |case: &Case| -> Result<Vec<DiffLine>, String> { Ok(classify_diff(case.arg_str(0)?)) },
        );
    }

    #[test]
    fn untracked_diff_parity() {
        replay("changesModel/untrackedDiff", |case: &Case| -> Result<String, String> {
            Ok(untracked_diff(case.arg_str(0)?))
        });
    }
}
