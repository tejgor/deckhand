//! Port of detailTexts.ts.
//!
//! Pure text builders for the scrollable details panes; the details pane wraps them and shows invisible characters.
//! The cleanup view's file marks and lost-file count (cleanupView.tsx) are here too, since the DELETE screen text
//! uses them; the cleanup pane renders with them.

use serde::Serialize;
use serde_with::skip_serializing_none;

use deckhand_protocol::data::{CleanupFileState, CleanupInspection, ProjectInfo, WorkspaceSummary, WorktreeLinks};

/// TS: cleanupView.tsx CLEANUP_FILE_LEGEND.
pub const CLEANUP_FILE_LEGEND: &str = "M changed · ? untracked · ! ignored";

/// TS: cleanupView.tsx CLEANUP_FILE_MARKS.
pub const CLEANUP_FILE_MARKS: [(CleanupFileState, &str); 3] = [
    (CleanupFileState::Changed, "M"),
    (CleanupFileState::Untracked, "?"),
    (CleanupFileState::Ignored, "!"),
];

/// TS: cleanupView.tsx CLEANUP_FILE_MARKS[state].
pub fn cleanup_file_mark(state: CleanupFileState) -> &'static str {
    let _ = state;
    todo!("P12")
}

/// TS: cleanupView.tsx lostFileCount (every file the inspection counted; its `files` list may be cut).
pub fn lost_file_count(inspection: &CleanupInspection) -> i64 {
    let _ = inspection;
    todo!("P12")
}

/// TS: detailTexts.ts trustReviewText (`purpose`: what is about to run or be created from the repository).
pub fn trust_review_text(project: &ProjectInfo, purpose: Option<&str>) -> String {
    let _ = (project, purpose);
    todo!("P12")
}

/// TS: detailTexts.ts cleanupOverrideText (the typed-DELETE screen of x on a session).
pub fn cleanup_override_text(inspection: Option<&CleanupInspection>) -> String {
    let _ = inspection;
    todo!("P12")
}

/// TS: detailTexts.ts ProjectAction (one entry of the e list; `fallback`: the global command run when the review is
/// skipped).
#[skip_serializing_none]
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectAction {
    pub name: String,
    pub command: String,
    pub needs_trust: bool,
    pub fallback: Option<String>,
}

/// TS: detailTexts.ts projectActions.
pub fn project_actions(project: Option<&ProjectInfo>) -> Vec<ProjectAction> {
    let _ = project;
    todo!("P12")
}

/// TS: detailTexts.ts workspaceSummaryText options.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WorkspaceSummaryOptions {
    pub creating_pr: bool,
    pub links: Option<WorktreeLinks>,
}

/// TS: detailTexts.ts workspaceSummaryText (`pr_loading` defaults to false).
pub fn workspace_summary_text(
    summary: Option<&WorkspaceSummary>,
    pr_loading: bool,
    options: &WorkspaceSummaryOptions,
) -> String {
    let _ = (summary, pr_loading, options);
    todo!("P12")
}

/// TS: detailTexts.ts createPrConfirmText.
pub fn create_pr_confirm_text(summary: &WorkspaceSummary) -> String {
    let _ = summary;
    todo!("P12")
}
