//! The merge confirmation (m) and the conflict result view as pure data; the app renders them. Widths are TS
//! `.length` (UTF-16 units).

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;

use crate::display::{Color, HINT_SEPARATOR, HintPart, THEME, compact_path, fit_hint, truncate};
use crate::jsstr::{js_len, js_trim_end};
use crate::sidebar_model::format_age;
use deckhand_protocol::data::{MergeCommit, MergePreview, TargetOperation, WorktreeMergeResult};

/// TS: mergeModel.ts MergeLine.
#[skip_serializing_none]
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct MergeLine {
    pub text: String,
    pub color: Option<Color>,
    pub bold: Option<bool>,
}

/// TS: mergeModel.ts MergeNoteEntry.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MergeNoteEntry {
    pub key: String,
    pub title: String,
    pub lines: Vec<String>,
}

/// git.ts operationProblem (why a merge cannot start in a target with an operation in progress). Here so the merge
/// screen can use it without deckhand-sys; `deckhand_sys::git` reuses it.
pub fn operation_problem(operation: TargetOperation) -> String {
    if operation == TargetOperation::Conflicts {
        "The target has unresolved conflicts: resolve and commit them first".to_owned()
    } else {
        format!("A {operation} is in progress in the target: finish or abort it first")
    }
}

/// TS: mergeModel.ts plural (`1 file`, `2 files`).
pub fn plural(count: i64, word: &str) -> String {
    format!("{count} {word}{}", if count == 1 { "" } else { "s" })
}

/// TS: mergeModel.ts mergeOptions.
pub fn merge_options(preview: Option<&MergePreview>) -> Vec<String> {
    let into = preview
        .and_then(|preview| preview.target_branch.as_deref())
        .unwrap_or("the current branch");
    vec![
        format!("Merge into {into} without committing"),
        format!("Squash merge into {into} without committing"),
        "Cancel".to_owned(),
    ]
}

/// TS: mergeModel.ts surprisingTarget (not the main checkout, or not on the default branch).
pub fn surprising_target(preview: &MergePreview) -> bool {
    let target = preview.target_branch.as_deref().filter(|branch| !branch.is_empty());
    !preview.target_is_main
        || target.is_none()
        || preview
            .default_branch
            .as_deref()
            .is_some_and(|default| preview.target_branch.as_deref() != Some(default))
}

/// TS: mergeModel.ts overlappingFiles.
pub fn overlapping_files(preview: &MergePreview, commit_first: bool) -> Vec<String> {
    let uncommitted: &[String] = if commit_first && preview.uncommitted != 0 {
        &preview.overlap.uncommitted
    } else {
        &[]
    };
    let mut seen = HashSet::new();
    preview
        .overlap
        .committed
        .iter()
        .chain(uncommitted)
        .filter(|file| seen.insert(file.as_str()))
        .cloned()
        .collect()
}

/// TS: mergeModel.ts MergeConfirmInput `openItems`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenItemsToggle {
    pub titles: Vec<String>,
    /// A missing flag is off (JS falsy).
    #[serde(default)]
    pub send: bool,
}

/// TS: mergeModel.ts MergeConfirmInput.
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MergeConfirmInput {
    /// The session's display title.
    pub title: String,
    pub preview: Option<MergePreview>,
    pub preview_error: Option<String>,
    /// Commit the source worktree's uncommitted files first (the toggle; on by default). Missing: off (JS falsy).
    #[serde(default)]
    pub commit_first: bool,
    /// The commit message that would be used: the session's title.
    pub commit_message: String,
    /// A merge attempt that failed; nothing was merged.
    pub error: Option<String>,
    pub notes: Vec<MergeNoteEntry>,
    /// The worktree's open task: merging ticks it.
    pub tasks: Option<Vec<String>>,
    /// Open checklist items of the worktree's note, sent to the backlog with the merge unless `send` is off.
    pub open_items: Option<OpenItemsToggle>,
    /// Content columns (inside the border and padding).
    pub width: usize,
    /// The pane's rows, border included.
    pub height: usize,
    /// Not in the TS: the time now (epoch milliseconds), for each commit's age; missing, commits show no age.
    #[serde(default)]
    pub now: Option<i64>,
}

/// TS: mergeModel.ts MergeConfirmLayout.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct MergeConfirmLayout {
    pub title: String,
    /// Target, warnings, summary, commits and the uncommitted toggle.
    pub details: Vec<MergeLine>,
    /// The open-items row (selectable; space switches it), when there are any.
    pub toggles: Vec<String>,
    pub options: Vec<String>,
    pub hint: String,
    /// A failed attempt's output, after the hint.
    pub error: Vec<MergeLine>,
    /// The Notes section (its header first), last; empty when it does not fit.
    pub notes: Vec<MergeLine>,
    /// Not in the TS: which rows of `details` are the commit subjects (what a dialog scrolls; the rows before them,
    /// the target and summary, and after them, the uncommitted toggle and warnings, stay in view).
    #[serde(skip)]
    pub commit_rows: std::ops::Range<usize>,
}

/// TS: mergeModel.ts mergeConfirmLayout.
///
/// The confirmation screen in `height` rows: title, details, a blank, the three options, a blank, the hint, then a
/// failed attempt's output and the notes. When rows are short the notes shrink first (and go), then the error output,
/// then the commit subjects; the target line, warnings, summary and toggle stay.
pub fn merge_confirm_layout(input: &MergeConfirmInput) -> MergeConfirmLayout {
    let preview = input.preview.as_ref();
    let width = input.width;
    let js_width = to_js(width);
    let fit = |text: &str| truncate(text, width);
    let line = |text: String, color: Option<Color>| MergeLine {
        text,
        color,
        bold: None,
    };
    let mut head: Vec<MergeLine> = Vec::new();
    let mut tail: Vec<MergeLine> = Vec::new();
    let mut commits: Vec<MergeLine> = Vec::new();
    match preview {
        None => head.push(match input.preview_error.as_deref().filter(|error| !error.is_empty()) {
            Some(error) => line(fit(&format!("Preview unavailable: {error}")), Some(THEME.warn)),
            None => line("Checking what will be merged…".to_owned(), Some(THEME.muted)),
        }),
        Some(preview) => {
            let branch = preview.target_branch.as_deref().unwrap_or("(detached)");
            let prefix = format!("Into {branch} · ");
            let room = (js_width - to_js(js_len(&prefix))).max(8);
            let target = compact_path(&preview.target_root, to_index(room));
            head.push(line(
                fit(&format!("{prefix}{target}")),
                Some(if surprising_target(preview) {
                    THEME.warn
                } else {
                    THEME.muted
                }),
            ));
            if let Some(operation) = preview.in_progress {
                head.push(line(
                    fit(&format!("⚠ {}", operation_problem(operation))),
                    Some(THEME.warn),
                ));
            }
            let extra = if input.commit_first && preview.uncommitted != 0 {
                format!(" · +{}", plural(preview.uncommitted, "uncommitted file"))
            } else {
                String::new()
            };
            head.push(if preview.commit_count != 0 {
                line(
                    fit(&format!(
                        "{} · {} +{} −{}{extra}",
                        plural(preview.commit_count, "commit"),
                        plural(preview.diff.files, "file"),
                        preview.diff.insertions,
                        preview.diff.deletions,
                    )),
                    None,
                )
            } else {
                let color = if extra.is_empty() { Some(THEME.muted) } else { None };
                line(fit(&format!("No new commits{extra}")), color)
            });
            commits = preview
                .commits
                .iter()
                .map(|commit| line(fit(&commit_text(commit, input.now)), Some(THEME.muted)))
                .collect();
            let more = preview.commit_count - to_js(preview.commits.len());
            if !commits.is_empty() && more > 0 {
                commits.push(line(fit(&format!("  +{more} more")), Some(THEME.muted)));
            }
            if preview.uncommitted != 0 {
                tail.push(line(fit(&plural(preview.uncommitted, "uncommitted file")), None));
                // The message gives way first, so the line always says what happens.
                let toggle = |quoted: &str| {
                    if input.commit_first {
                        format!("☑ commit them first ({quoted})")
                    } else {
                        format!("☐ commit them first ({quoted}) · they stay in the worktree")
                    }
                };
                let room = (js_width - to_js(js_len(&toggle("\"\"")))).max(4);
                let quoted = format!("\"{}\"", truncate(&input.commit_message, to_index(room)));
                let color = if input.commit_first { THEME.active } else { THEME.muted };
                tail.push(line(fit(&toggle(&quoted)), Some(color)));
            }
            let tasks = input.tasks.as_deref().unwrap_or_default();
            if let [task] = tasks {
                tail.push(line(
                    fit(&format!("◆ Task \"{task}\" will be marked done")),
                    Some(THEME.accent_soft),
                ));
            } else if tasks.len() > 1 {
                tail.push(line(
                    fit(&format!(
                        "◆ {} tasks will be marked done: {}",
                        tasks.len(),
                        tasks.join(", ")
                    )),
                    Some(THEME.accent_soft),
                ));
            }
            let overlap = overlapping_files(preview, input.commit_first);
            if !overlap.is_empty() {
                let shown = overlap
                    .iter()
                    .take(3)
                    .map(String::as_str)
                    .collect::<Vec<_>>()
                    .join(", ");
                let ellipsis = if overlap.len() > 3 { ", …" } else { "" };
                tail.push(line(
                    fit(&format!(
                        "⚠ Target has uncommitted changes in {} the merge touches: {shown}{ellipsis}",
                        plural(to_js(overlap.len()), "file"),
                    )),
                    Some(THEME.warn),
                ));
            }
        }
    }
    // The note's open items (the task's unfinished steps): a selectable row space switches.
    let items = preview.and(input.open_items.as_ref());
    let toggles: Vec<String> = match items {
        Some(items) if !items.titles.is_empty() => vec![fit(&format!(
            "{} {} {}: {}",
            if items.send { "☑" } else { "☐" },
            plural(to_js(items.titles.len()), "open note item"),
            if items.send {
                "→ backlog"
            } else {
                "· stay in the note"
            },
            items.titles.join(", "),
        ))],
        _ => Vec::new(),
    };
    // Space switches the selected row, else commit-first: one hint for both.
    let space = if !toggles.is_empty() {
        Some(("space toggle", "space toggle"))
    } else if preview.is_some_and(|preview| preview.uncommitted != 0) {
        Some(("space commit first", "space toggle"))
    } else {
        None
    };
    let mut parts = vec![HintPart::from("enter choose")];
    if let Some((text, short)) = space {
        parts.push(HintPart {
            text: text.to_owned(),
            short: Some(short.to_owned()),
            drop: None,
        });
    }
    parts.push(HintPart::from("esc cancel"));
    let hint = fit_hint(&parts, width, HINT_SEPARATOR);
    // Border (2), title, the blank and three options, the blank and the hint.
    let mut free =
        (to_js(input.height) - 2 - 1 - 4 - 2 - to_js(head.len()) - to_js(tail.len()) - to_js(toggles.len())).max(0);
    // Commit subjects before the error and the notes; when cut, the last row shown says how many more there are.
    let shown = to_index(free).min(commits.len());
    free -= to_js(shown);
    let mut shown_commits: Vec<MergeLine> = commits[..shown].to_vec();
    if shown < commits.len() && shown > 0 {
        let commit_count = preview.map_or(0, |preview| preview.commit_count);
        let hidden = commit_count - (to_js(shown) - 1);
        shown_commits.pop();
        shown_commits.push(line(fit(&format!("  +{hidden} more")), Some(THEME.muted)));
    }
    let error_lines: Vec<MergeLine> = input
        .error
        .as_deref()
        .unwrap_or_default()
        .split('\n')
        .map(js_trim_end)
        .filter(|text| !text.is_empty())
        .enumerate()
        .map(|(index, text)| {
            let text = if index > 0 {
                format!("  {text}")
            } else {
                text.to_owned()
            };
            // Whole: the dialog wraps it (a hook's output is what explains the failure).
            line(text, Some(THEME.error))
        })
        .collect();
    // A blank row before the error output, then its lines (the last ones matter most for a hook's failure).
    let mut error: Vec<MergeLine> = Vec::new();
    if !error_lines.is_empty() && free >= 2 {
        free -= 1;
        let room = to_index(free);
        error = if error_lines.len() <= room {
            error_lines
        } else {
            // `[first, ...slice(-(free - 1))].slice(0, free)`: `slice(-0)` is the whole list.
            let last = room - 1;
            let tail_from = if last == 0 { 0 } else { error_lines.len() - last };
            let mut lines = vec![error_lines[0].clone()];
            lines.extend_from_slice(&error_lines[tail_from..]);
            lines.truncate(room);
            lines
        };
        free -= to_js(error.len());
    }
    // The notes: a blank, the header and at least one line, else nothing.
    let note_lines: Vec<MergeLine> = if input.notes.is_empty() {
        vec![line(
            fit("No notes for this worktree, this session or its sub-sessions."),
            Some(THEME.muted),
        )]
    } else {
        input
            .notes
            .iter()
            .flat_map(|entry| {
                std::iter::once(MergeLine {
                    text: entry.title.clone(),
                    color: Some(THEME.muted),
                    bold: Some(true),
                })
                .chain(
                    entry
                        .lines
                        .iter()
                        // Whole: the dialog wraps note lines rather than cutting them.
                        .map(|text| line(format!("  {text}"), Some(THEME.muted))),
                )
            })
            .collect()
    };
    let mut notes: Vec<MergeLine> = Vec::new();
    if free >= 3 {
        let room = to_index(free - 2);
        let shown: Vec<MergeLine> = if note_lines.len() <= room {
            note_lines
        } else {
            let mut shown = note_lines[..room - 1].to_vec();
            shown.push(line(
                format!("+{} more lines", note_lines.len() - room + 1),
                Some(THEME.muted),
            ));
            shown
        };
        notes.push(MergeLine {
            text: "Notes".to_owned(),
            color: Some(THEME.accent_soft),
            bold: Some(true),
        });
        notes.extend(shown);
    }
    let mut details = head;
    let commits_start = details.len();
    details.extend(shown_commits);
    let commit_rows = commits_start..details.len();
    details.extend(tail);
    MergeConfirmLayout {
        title: fit(&format!("Merge \"{}\"?", input.title)),
        details,
        toggles,
        options: merge_options(preview),
        hint,
        error,
        notes,
        commit_rows,
    }
}

/// A Rust count as the JS number it stands for.
/// A commit's row: its short hash and age (right-aligned, so the subjects line up), then its subject.
fn commit_text(commit: &MergeCommit, now: Option<i64>) -> String {
    match commit {
        MergeCommit::Subject(subject) => format!("  {subject}"),
        MergeCommit::Detail { hash, time, subject } => match now {
            #[allow(clippy::cast_precision_loss)]
            Some(now) => {
                let age = format_age((now - time.saturating_mul(1000)).max(0) as f64);
                format!("  {hash} {age:>4}  {subject}")
            }
            None => format!("  {hash}  {subject}"),
        },
    }
}

fn to_js(value: usize) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

/// A JS number as a Rust width or count (negative: 0).
fn to_index(value: i64) -> usize {
    usize::try_from(value.max(0)).unwrap_or(usize::MAX)
}

/// TS: mergeModel.ts mergeConfirmRows (rows the confirmation needs, border included).
pub fn merge_confirm_rows(layout: &MergeConfirmLayout) -> usize {
    let error = if layout.error.is_empty() {
        0
    } else {
        layout.error.len() + 1
    };
    let notes = if layout.notes.is_empty() {
        0
    } else {
        layout.notes.len() + 1
    };
    2 + 1 + layout.details.len() + layout.toggles.len() + 1 + layout.options.len() + 1 + 1 + error + notes
}

/// TS: mergeModel.ts ConflictView choice.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct ConflictChoice {
    pub key: String,
    pub text: String,
}

/// TS: mergeModel.ts ConflictView.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct ConflictView {
    pub title: String,
    pub files: Vec<String>,
    pub choices: Vec<ConflictChoice>,
}

/// TS: mergeModel.ts conflictView (reads the result's `conflicts` and `conflictCount`).
///
/// The result of a conflicted merge: what conflicts, and the only two choices (keep it in progress, or abort it).
pub fn conflict_view(result: &WorktreeMergeResult, width: usize) -> ConflictView {
    let files: &[String] = result.conflicts.as_deref().unwrap_or_default();
    let listed = to_js(files.len());
    let total = result.conflict_count.unwrap_or(listed).max(listed);
    // Every file listed (the dialog scrolls); `+N more` only for files the result does not name.
    let shown = files;
    let mut lines: Vec<String> = shown.iter().map(|file| truncate(&format!("  {file}"), width)).collect();
    if total > to_js(shown.len()) {
        lines.push(format!("  +{} more", total - to_js(shown.len())));
    }
    ConflictView {
        title: truncate(&format!("Merged with conflicts in {}", plural(total, "file")), width),
        files: lines,
        choices: vec![
            ConflictChoice {
                key: "enter".to_owned(),
                text: "keep it: resolve it in your editor or the Git tab (esc too)".to_owned(),
            },
            ConflictChoice {
                key: "x".to_owned(),
                text: "abort the merge".to_owned(),
            },
        ],
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use deckhand_testkit::{Case, replay, tags};
    use serde_json::{Value, json};

    /// `compactPath` reads `$HOME`: tests that set it hold this lock (TS `withEnv`).
    static HOME_LOCK: Mutex<()> = Mutex::new(());

    fn with_home<T>(home: &str, run: impl FnOnce() -> T) -> T {
        let _guard = HOME_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let saved = std::env::var_os("HOME");
        // SAFETY: every test that reads or writes HOME in this module holds HOME_LOCK.
        unsafe { std::env::set_var("HOME", home) };
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(run));
        match saved {
            // SAFETY: as above.
            Some(saved) => unsafe { std::env::set_var("HOME", saved) },
            // SAFETY: as above.
            None => unsafe { std::env::remove_var("HOME") },
        }
        result.unwrap_or_else(|panic| std::panic::resume_unwind(panic))
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

    /// The test's `preview(fields)`.
    fn preview(fields: Value) -> MergePreview {
        let base = json!({
            "sourceRef": "feat/auth", "sourceSha": "a".repeat(40), "targetRoot": "/home/me/code/app", "targetBranch": "main",
            "targetIsMain": true, "defaultBranch": "main", "commitCount": 8,
            "commits": ["add login form", "wire the session cookie", "fix tests", "rename handler", "docs", "lint"],
            "diff": {"files": 5, "insertions": 120, "deletions": 14}, "uncommitted": 4,
            "overlap": {"committed": [], "uncommitted": []},
        });
        serde_json::from_value(merged(base, fields)).expect("a preview literal")
    }

    /// The test's `input(fields)` (without fields: the struct update syntax sets them).
    fn input() -> MergeConfirmInput {
        MergeConfirmInput {
            title: "auth refactor".into(),
            preview: Some(preview(json!({}))),
            preview_error: None,
            commit_first: true,
            commit_message: "auth refactor".into(),
            error: None,
            notes: Vec::new(),
            tasks: None,
            open_items: None,
            width: 60,
            height: 40,
            now: None,
        }
    }

    fn texts(lines: &[MergeLine]) -> Vec<&str> {
        lines.iter().map(|line| line.text.as_str()).collect()
    }

    fn strings(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| (*item).to_owned()).collect()
    }

    #[test]
    fn confirmation_target_summary_toggle_options_hint() {
        with_home("/home/me", || {
            let layout = merge_confirm_layout(&MergeConfirmInput {
                notes: vec![MergeNoteEntry {
                    key: "w".into(),
                    title: "Worktree notes · feat/auth".into(),
                    lines: strings(&["- [ ] review", "ship it"]),
                }],
                ..input()
            });
            assert_eq!(layout.title, "Merge \"auth refactor\"?");
            assert_eq!(
                texts(&layout.details),
                [
                    "Into main · ~/code/app",
                    "8 commits · 5 files +120 −14 · +4 uncommitted files",
                    "  add login form",
                    "  wire the session cookie",
                    "  fix tests",
                    "  rename handler",
                    "  docs",
                    "  lint",
                    "  +2 more",
                    "4 uncommitted files",
                    "☑ commit them first (\"auth refactor\")",
                ]
            );
            // The expected target (main checkout, default branch) is muted; the toggle is highlighted while on.
            assert_eq!(layout.details[0].color, Some(THEME.muted));
            assert_eq!(layout.details.last().and_then(|line| line.color), Some(THEME.active));
            assert_eq!(
                layout.options,
                [
                    "Merge into main without committing",
                    "Squash merge into main without committing",
                    "Cancel"
                ]
            );
            assert_eq!(layout.hint, "enter choose • space commit first • esc cancel");
            assert_eq!(
                texts(&layout.notes),
                ["Notes", "Worktree notes · feat/auth", "  - [ ] review", "  ship it"]
            );
            assert!(merge_confirm_rows(&layout) <= 40);

            // Toggle off: the summary has no uncommitted part and the files stay in the worktree.
            let off = merge_confirm_layout(&MergeConfirmInput {
                commit_first: false,
                width: 72,
                ..input()
            });
            assert_eq!(off.details[1].text, "8 commits · 5 files +120 −14");
            assert_eq!(
                off.details.last().map(|line| line.text.as_str()),
                Some("☐ commit them first (\"auth refactor\") · they stay in the worktree")
            );
            // Narrow: the message is cut, never what happens to the files.
            let narrow = merge_confirm_layout(&MergeConfirmInput {
                commit_first: false,
                width: 60,
                ..input()
            });
            assert_eq!(
                narrow.details.last().map(|line| line.text.as_str()),
                Some("☐ commit them first (\"auth re…\") · they stay in the worktree")
            );
            // Nothing uncommitted: no toggle and no space in the hint; no commits either: says so.
            let clean = merge_confirm_layout(&MergeConfirmInput {
                preview: Some(preview(json!({"uncommitted": 0, "commitCount": 1, "commits": ["one"],
                    "diff": {"files": 1, "insertions": 1, "deletions": 0}}))),
                ..input()
            });
            assert_eq!(
                texts(&clean.details),
                ["Into main · ~/code/app", "1 commit · 1 file +1 −0", "  one"]
            );
            assert_eq!(clean.hint, "enter choose • esc cancel");
            let only_uncommitted = merge_confirm_layout(&MergeConfirmInput {
                preview: Some(preview(json!({"commitCount": 0, "commits": [],
                    "diff": {"files": 0, "insertions": 0, "deletions": 0}, "uncommitted": 1}))),
                ..input()
            });
            assert_eq!(
                texts(&only_uncommitted.details),
                [
                    "Into main · ~/code/app",
                    "No new commits · +1 uncommitted file",
                    "1 uncommitted file",
                    "☑ commit them first (\"auth refactor\")"
                ]
            );
            let nothing = merge_confirm_layout(&MergeConfirmInput {
                preview: Some(preview(json!({"commitCount": 0, "commits": [], "uncommitted": 0}))),
                ..input()
            });
            assert_eq!(nothing.details[1].text, "No new commits");
            // Loading and a failed preview.
            let loading = merge_confirm_layout(&MergeConfirmInput {
                preview: None,
                ..input()
            });
            assert_eq!(texts(&loading.details), ["Checking what will be merged…"]);
            let failed = merge_confirm_layout(&MergeConfirmInput {
                preview: None,
                preview_error: Some("not a git repository".into()),
                ..input()
            });
            assert_eq!(texts(&failed.details), ["Preview unavailable: not a git repository"]);
            assert_eq!(
                loading.options,
                [
                    "Merge into the current branch without committing",
                    "Squash merge into the current branch without committing",
                    "Cancel"
                ]
            );
        });
    }

    #[test]
    fn commits_show_their_hash_and_age_with_the_subjects_lined_up() {
        with_home("/home/me", || {
            let now = 1_700_000_000_000;
            let at = |ms_ago: i64| (now - ms_ago) / 1000;
            let layout = merge_confirm_layout(&MergeConfirmInput {
                preview: Some(preview(json!({"commitCount": 3, "commits": [
                    {"hash": "a1b2c3d", "time": at(30_000), "subject": "fix tests"},
                    {"hash": "e4f5a6b", "time": at(3 * 3_600_000), "subject": "wire the session cookie"},
                    {"hash": "c7d8e9f", "time": at(12 * 86_400_000), "subject": "add login form"},
                ]}))),
                now: Some(now),
                ..input()
            });
            let commits = &layout.details[layout.commit_rows.clone()];
            assert_eq!(
                texts(commits),
                [
                    "  a1b2c3d  now  fix tests",
                    "  e4f5a6b   3h  wire the session cookie",
                    "  c7d8e9f  12d  add login form",
                ]
            );
            // Without the time now: the hash alone. A bare subject (what the TS sent) still shows.
            let layout = merge_confirm_layout(&MergeConfirmInput {
                preview: Some(preview(json!({"commitCount": 2, "commits": [
                    {"hash": "a1b2c3d", "time": at(0), "subject": "fix tests"}, "lint",
                ]}))),
                ..input()
            });
            assert_eq!(
                texts(&layout.details[layout.commit_rows.clone()]),
                ["  a1b2c3d  fix tests", "  lint"]
            );
        });
    }

    #[test]
    fn confirmation_warns_surprising_target_operations_and_overlap() {
        with_home("/home/me", || {
            // Not the main checkout, or not the default branch: warn.
            for fields in [
                json!({"targetIsMain": false, "targetRoot": "/home/me/wt/other"}),
                json!({"targetBranch": "release"}),
                json!({"targetBranch": null}),
            ] {
                let layout = merge_confirm_layout(&MergeConfirmInput {
                    preview: Some(preview(fields.clone())),
                    ..input()
                });
                assert_eq!(layout.details[0].color, Some(THEME.warn), "{fields}");
                assert!(surprising_target(&preview(fields)));
            }
            let other = merge_confirm_layout(&MergeConfirmInput {
                preview: Some(preview(
                    json!({"targetIsMain": false, "targetRoot": "/home/me/wt/other"}),
                )),
                ..input()
            });
            assert_eq!(other.details[0].text, "Into main · ~/wt/other");
            let detached = merge_confirm_layout(&MergeConfirmInput {
                preview: Some(preview(json!({"targetBranch": null}))),
                ..input()
            });
            assert_eq!(detached.details[0].text, "Into (detached) · ~/code/app");
            // No default branch known: only the checkout decides.
            assert!(!surprising_target(&preview(
                json!({"defaultBranch": null, "targetBranch": "trunk"})
            )));
            // A long path is compacted to fit after the branch.
            let long = merge_confirm_layout(&MergeConfirmInput {
                width: 30,
                preview: Some(preview(json!({"targetRoot": "/home/me/projects/clients/acme/app"}))),
                ..input()
            });
            assert_eq!(long.details[0].text, "Into main · …/acme/app");

            let busy = merge_confirm_layout(&MergeConfirmInput {
                width: 80,
                preview: Some(preview(json!({"inProgress": "rebase"}))),
                ..input()
            });
            assert_eq!(
                (busy.details[1].text.as_str(), busy.details[1].color),
                (
                    "⚠ A rebase is in progress in the target: finish or abort it first",
                    Some(THEME.warn)
                )
            );
            let conflicts = merge_confirm_layout(&MergeConfirmInput {
                width: 80,
                preview: Some(preview(json!({"inProgress": "conflicts"}))),
                ..input()
            });
            assert_eq!(
                conflicts.details[1].text,
                "⚠ The target has unresolved conflicts: resolve and commit them first"
            );

            // Target edits the commits touch always count; those the uncommitted files touch only when they are
            // committed first.
            let overlapping =
                preview(json!({"overlap": {"committed": ["src/a.ts"], "uncommitted": ["src/b.ts", "src/a.ts"]}}));
            assert_eq!(overlapping_files(&overlapping, true), ["src/a.ts", "src/b.ts"]);
            assert_eq!(overlapping_files(&overlapping, false), ["src/a.ts"]);
            let warned = merge_confirm_layout(&MergeConfirmInput {
                width: 100,
                preview: Some(overlapping.clone()),
                ..input()
            });
            let last = warned.details.last().expect("a details line");
            assert_eq!(
                (last.text.as_str(), last.color),
                (
                    "⚠ Target has uncommitted changes in 2 files the merge touches: src/a.ts, src/b.ts",
                    Some(THEME.warn)
                )
            );
            let committed_only = merge_confirm_layout(&MergeConfirmInput {
                width: 100,
                commit_first: false,
                preview: Some(overlapping),
                ..input()
            });
            assert_eq!(
                committed_only.details.last().map(|line| line.text.as_str()),
                Some("⚠ Target has uncommitted changes in 1 file the merge touches: src/a.ts")
            );
            let plain = merge_confirm_layout(&input());
            assert!(!plain.details.iter().any(|line| line.text.starts_with('⚠')));
        });
    }

    #[test]
    fn confirmation_is_height_budgeted() {
        // The TS test runs with the real HOME, which /home/me is not.
        with_home("/nonexistent-home", || {
            let notes = vec![MergeNoteEntry {
                key: "s".into(),
                title: "auth refactor".into(),
                lines: (1..=12).map(|index| format!("note {index}")).collect(),
            }];
            let error = "Commit failed in the worktree, so nothing was merged:\nhusky - pre-commit\nlint: 3 problems\nsrc/a.ts:1 no-unused-vars\nsrc/b.ts:9 eqeqeq";
            let at = |height: usize| {
                merge_confirm_layout(&MergeConfirmInput {
                    notes: notes.clone(),
                    error: Some(error.into()),
                    height,
                    ..input()
                })
            };
            // Never more rows than the pane, down to the essentials (13 rows here).
            for height in 13..=45 {
                assert!(merge_confirm_rows(&at(height)) <= height, "{height}");
            }
            // Plenty of room: everything, the notes complete.
            let roomy = at(42);
            assert_eq!([roomy.notes.len(), roomy.error.len(), roomy.details.len()], [14, 5, 11]);
            // Less: the notes are cut (with a count), then gone, before anything else.
            assert_eq!(texts(&at(40).notes).last(), Some(&"+2 more lines"));
            assert_eq!(merge_confirm_rows(&at(40)), 40);
            let less = at(28);
            assert_eq!([less.notes.len(), less.error.len(), less.details.len()], [0, 5, 11]);
            // Then the error keeps its first and last lines (a hook's verdict comes last), then goes.
            assert_eq!(
                texts(&at(24).error),
                [
                    "Commit failed in the worktree, so nothing was merged:",
                    "  src/a.ts:1 no-unused-vars",
                    "  src/b.ts:9 eqeqeq"
                ]
            );
            let gone = at(21);
            assert_eq!([gone.error.len(), gone.details.len()], [0, 11]);
            // Then the commit subjects shrink; the last row shown counts the rest.
            let tight = at(15);
            assert_eq!(
                texts(&tight.details),
                [
                    "Into main · /home/me/code/app",
                    "8 commits · 5 files +120 −14 · +4 uncommitted files",
                    "  add login form",
                    "  +7 more",
                    "4 uncommitted files",
                    "☑ commit them first (\"auth refactor\")"
                ]
            );
            assert_eq!(merge_confirm_rows(&tight), 15);
            // The essentials stay even when nothing else fits.
            assert_eq!(
                texts(&at(9).details),
                [
                    "Into main · /home/me/code/app",
                    "8 commits · 5 files +120 −14 · +4 uncommitted files",
                    "4 uncommitted files",
                    "☑ commit them first (\"auth refactor\")"
                ]
            );
        });
    }

    /// A merge result with only `conflicts` / `conflictCount` set (TS `Pick<...>`).
    fn conflicted(conflicts: Option<Vec<String>>, conflict_count: Option<i64>) -> WorktreeMergeResult {
        WorktreeMergeResult {
            mode: deckhand_protocol::data::WorktreeMergeMode::Merge,
            source_ref: String::new(),
            source_sha: None,
            target_branch: String::new(),
            skipped: None,
            conflicted: None,
            conflicts,
            conflict_count,
            committed: None,
            reason: None,
            stdout: String::new(),
            stderr: String::new(),
            sent_items: None,
        }
    }

    #[test]
    fn conflict_view_counts_files_and_choices() {
        let view = conflict_view(
            &conflicted(Some(strings(&["src/a.ts", "src/b.ts", "README.md"])), Some(3)),
            60,
        );
        assert_eq!(
            view,
            ConflictView {
                title: "Merged with conflicts in 3 files".into(),
                files: strings(&["  src/a.ts", "  src/b.ts", "  README.md"]),
                choices: vec![
                    ConflictChoice {
                        key: "enter".into(),
                        text: "keep it: resolve it in your editor or the Git tab (esc too)".into()
                    },
                    ConflictChoice {
                        key: "x".into(),
                        text: "abort the merge".into()
                    },
                ],
            }
        );
        let many = conflict_view(
            &conflicted(Some((0..9).map(|index| format!("f{index}.ts")).collect()), Some(240)),
            60,
        );
        // Every listed file (the dialog scrolls), then how many the result does not name.
        assert_eq!(many.files.len(), 10);
        assert_eq!(many.files[8], "  f8.ts");
        assert_eq!(many.files[9], "  +231 more");
        assert_eq!(many.title, "Merged with conflicts in 240 files");
        assert_eq!(
            conflict_view(&conflicted(Some(strings(&["only.ts"])), Some(1)), 60).title,
            "Merged with conflicts in 1 file"
        );
        assert_eq!(
            conflict_view(&conflicted(Some(strings(&["a", "b", "c", "d", "e"])), None), 60)
                .files
                .len(),
            5
        );
    }

    #[test]
    fn confirmation_names_task_and_offers_open_items() {
        let layout = merge_confirm_layout(&MergeConfirmInput {
            tasks: Some(strings(&["Add OAuth"])),
            open_items: Some(OpenItemsToggle {
                titles: strings(&["Add tests", "Remove debug logging"]),
                send: true,
            }),
            height: 30,
            width: 100,
            ..input()
        });
        assert_eq!(
            texts(&layout.details).last(),
            Some(&"◆ Task \"Add OAuth\" will be marked done")
        );
        assert_eq!(
            layout.toggles,
            ["☑ 2 open note items → backlog: Add tests, Remove debug logging"]
        );
        assert_eq!(layout.hint, "enter choose • space toggle • esc cancel");
        let without_items = merge_confirm_layout(&MergeConfirmInput {
            tasks: Some(strings(&["Add OAuth"])),
            width: 100,
            ..input()
        });
        assert_eq!(without_items.hint, "enter choose • space commit first • esc cancel");
        assert!(merge_confirm_rows(&layout) <= 30);
        for height in 16..=30 {
            let layout = merge_confirm_layout(&MergeConfirmInput {
                open_items: Some(OpenItemsToggle {
                    titles: strings(&["a"]),
                    send: false,
                }),
                height,
                ..input()
            });
            assert!(merge_confirm_rows(&layout) <= height, "{height}");
        }
        // Until the preview arrives nothing is offered (the merge cannot be chosen yet either).
        let loading = merge_confirm_layout(&MergeConfirmInput {
            preview: None,
            open_items: Some(OpenItemsToggle {
                titles: strings(&["a"]),
                send: true,
            }),
            ..input()
        });
        assert!(loading.toggles.is_empty());
    }

    #[test]
    fn confirmation_open_items_row_switches() {
        let base = || MergeConfirmInput {
            title: "auth".into(),
            preview: Some(preview(json!({"commitCount": 1, "commits": ["feat"],
                "diff": {"files": 1, "insertions": 1, "deletions": 0}, "uncommitted": 0}))),
            commit_message: "auth".into(),
            width: 100,
            height: 30,
            ..input()
        };
        let send = merge_confirm_layout(&MergeConfirmInput {
            open_items: Some(OpenItemsToggle {
                titles: strings(&["Write the parser", "Handle tabs"]),
                send: true,
            }),
            ..base()
        });
        assert_eq!(
            send.toggles,
            ["☑ 2 open note items → backlog: Write the parser, Handle tabs"]
        );
        assert_eq!(send.hint, "enter choose • space toggle • esc cancel");
        let keep = merge_confirm_layout(&MergeConfirmInput {
            open_items: Some(OpenItemsToggle {
                titles: strings(&["Write the parser"]),
                send: false,
            }),
            ..base()
        });
        assert_eq!(
            keep.toggles,
            ["☐ 1 open note item · stay in the note: Write the parser"]
        );
        // Nothing before the preview arrives.
        let loading = merge_confirm_layout(&MergeConfirmInput {
            preview: None,
            open_items: Some(OpenItemsToggle {
                titles: strings(&["x"]),
                send: true,
            }),
            ..base()
        });
        assert!(loading.toggles.is_empty());
    }

    #[test]
    fn plural_and_operation_problem() {
        assert_eq!(plural(1, "file"), "1 file");
        assert_eq!(plural(0, "file"), "0 files");
        assert_eq!(plural(-1, "file"), "-1 files");
        assert_eq!(
            operation_problem(TargetOperation::CherryPick),
            "A cherry-pick is in progress in the target: finish or abort it first"
        );
    }

    // ── Parity replays (fixtures/cases/mergeModel) ──────────────────────────────────────────────────────────

    /// A recorded MergePreview: a mutation may drop `targetIsMain` (undefined: falsy).
    fn preview_value(value: &Value) -> Value {
        let mut value = tags::untag(value);
        if let Some(object) = value.as_object_mut() {
            object.entry("targetIsMain").or_insert(json!(false));
        }
        value
    }

    fn preview_arg(case: &Case, index: usize) -> Result<MergePreview, String> {
        serde_json::from_value(preview_value(case.arg(index))).map_err(|error| error.to_string())
    }

    /// A JS width as the port's `usize`: a negative width behaves as 0 everywhere TS uses it (`truncate`, the
    /// `Math.max` floors, `fitHint`, the height budget's `Math.max(0, ..)`).
    fn clamp_width(value: &mut Value) {
        if value.as_i64().is_some_and(|number| number < 0) {
            *value = json!(0);
        }
    }

    fn input_arg(case: &Case) -> Result<MergeConfirmInput, String> {
        let mut value = tags::untag(case.arg(0));
        let object = value.as_object_mut().ok_or("not an object")?;
        for key in ["width", "height"] {
            if let Some(number) = object.get_mut(key) {
                clamp_width(number);
            }
        }
        if let Some(preview) = object.get_mut("preview").filter(|preview| preview.is_object()) {
            *preview = preview_value(preview);
        }
        serde_json::from_value(value).map_err(|error| error.to_string())
    }

    /// A recorded layout (its colors are leaked to get the `'static` names the port uses).
    fn layout_arg(case: &Case) -> Result<MergeConfirmLayout, String> {
        let value = tags::untag(case.arg(0));
        let lines = |key: &str| -> Result<Vec<MergeLine>, String> {
            value
                .get(key)
                .and_then(Value::as_array)
                .ok_or(format!("{key} is not an array"))?
                .iter()
                .map(|line| {
                    Ok(MergeLine {
                        text: line.get("text").and_then(Value::as_str).ok_or("text")?.to_owned(),
                        color: line
                            .get("color")
                            .and_then(Value::as_str)
                            .map(|color| &*Box::leak(color.to_owned().into_boxed_str())),
                        bold: line.get("bold").and_then(Value::as_bool),
                    })
                })
                .collect()
        };
        let list = |key: &str| -> Result<Vec<String>, String> {
            serde_json::from_value(value.get(key).cloned().unwrap_or_default()).map_err(|error| error.to_string())
        };
        let text = |key: &str| value.get(key).and_then(Value::as_str).unwrap_or_default().to_owned();
        Ok(MergeConfirmLayout {
            title: text("title"),
            details: lines("details")?,
            toggles: list("toggles")?,
            options: list("options")?,
            hint: text("hint"),
            error: lines("error")?,
            notes: lines("notes")?,
            commit_rows: 0..0,
        })
    }

    #[test]
    fn merge_options_parity() {
        replay("mergeModel/mergeOptions", |case: &Case| -> Result<_, String> {
            let preview = if case.arg_is_undefined(0) {
                None
            } else {
                Some(preview_arg(case, 0)?)
            };
            Ok(merge_options(preview.as_ref()))
        });
    }

    #[test]
    fn surprising_target_parity() {
        replay("mergeModel/surprisingTarget", |case: &Case| {
            preview_arg(case, 0).map(|preview| surprising_target(&preview))
        });
    }

    #[test]
    fn overlapping_files_parity() {
        replay("mergeModel/overlappingFiles", |case: &Case| -> Result<_, String> {
            Ok(overlapping_files(&preview_arg(case, 0)?, case.arg_bool(1)?))
        });
    }

    #[test]
    fn merge_confirm_layout_parity() {
        with_home("/nonexistent-parity-home", || {
            replay("mergeModel/mergeConfirmLayout", |case: &Case| {
                input_arg(case).map(|input| merge_confirm_layout(&input))
            });
        });
    }

    #[test]
    fn merge_confirm_rows_parity() {
        replay("mergeModel/mergeConfirmRows", |case: &Case| {
            layout_arg(case).map(|layout| merge_confirm_rows(&layout))
        });
    }

    #[test]
    fn conflict_view_parity() {
        replay("mergeModel/conflictView", |case: &Case| -> Result<_, String> {
            let result = tags::untag(case.arg(0));
            let conflicts = result
                .get("conflicts")
                .map(|files| serde_json::from_value(files.clone()))
                .transpose()
                .map_err(|error| error.to_string())?;
            let count = result.get("conflictCount").and_then(Value::as_i64);
            let width = usize::try_from(case.arg_i64(1)?.max(0)).unwrap_or(usize::MAX);
            Ok(conflict_view(&conflicted(conflicts, count), width))
        });
    }
}
