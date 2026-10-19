//! Port of mergeModel.ts.
//!
//! The merge confirmation (m) and the conflict result view as pure data; the app renders them. Widths are TS
//! `.length` (UTF-16 units).

use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;

use crate::display::Color;
use deckhand_protocol::data::{MergePreview, TargetOperation, WorktreeMergeResult};

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
    let _ = operation;
    todo!("P4")
}

/// TS: mergeModel.ts plural (`1 file`, `2 files`).
pub fn plural(count: i64, word: &str) -> String {
    let _ = (count, word);
    todo!("P4")
}

/// TS: mergeModel.ts mergeOptions.
pub fn merge_options(preview: Option<&MergePreview>) -> Vec<String> {
    let _ = preview;
    todo!("P4")
}

/// TS: mergeModel.ts surprisingTarget (not the main checkout, or not on the default branch).
pub fn surprising_target(preview: &MergePreview) -> bool {
    let _ = preview;
    todo!("P4")
}

/// TS: mergeModel.ts overlappingFiles.
pub fn overlapping_files(preview: &MergePreview, commit_first: bool) -> Vec<String> {
    let _ = (preview, commit_first);
    todo!("P4")
}

/// TS: mergeModel.ts MergeConfirmInput `openItems`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenItemsToggle {
    pub titles: Vec<String>,
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
    /// Commit the source worktree's uncommitted files first (the toggle; on by default).
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
}

/// TS: mergeModel.ts mergeConfirmLayout.
pub fn merge_confirm_layout(input: &MergeConfirmInput) -> MergeConfirmLayout {
    let _ = input;
    todo!("P4")
}

/// TS: mergeModel.ts mergeConfirmRows (rows the confirmation needs, border included).
pub fn merge_confirm_rows(layout: &MergeConfirmLayout) -> usize {
    let _ = layout;
    todo!("P4")
}

/// TS: mergeModel.ts CONFLICT_FILES_SHOWN.
pub const CONFLICT_FILES_SHOWN: usize = 5;

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
pub fn conflict_view(result: &WorktreeMergeResult, width: usize) -> ConflictView {
    let _ = (result, width);
    todo!("P4")
}
