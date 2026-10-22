//! Pure text builders for the scrollable details panes; the details pane wraps them and shows invisible characters.
//! The cleanup view's file marks and lost-file count (cleanupView.tsx) are here too, since the DELETE screen text
//! uses them; the cleanup pane renders with them.

use serde::Serialize;
use serde_with::skip_serializing_none;

use indexmap::IndexMap;

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
    match state {
        CleanupFileState::Changed => "M",
        CleanupFileState::Untracked => "?",
        CleanupFileState::Ignored => "!",
    }
}

/// TS: cleanupView.tsx lostFileCount (every file the inspection counted; its `files` list may be cut).
pub fn lost_file_count(inspection: &CleanupInspection) -> i64 {
    let listed = inspection
        .files
        .as_ref()
        .map_or(0, |files| i64::try_from(files.len()).unwrap_or(i64::MAX));
    listed.max(inspection.dirty_files + inspection.untracked_files + inspection.ignored_files)
}

/// TS: detailTexts.ts trustReviewText (`purpose`: what is about to run or be created from the repository).
pub fn trust_review_text(project: &ProjectInfo, purpose: Option<&str>) -> String {
    let status = if project.needs_review {
        "Not trusted: until you trust it, its commands and worktree settings do not apply (your global defaults do; its agent/workspace suggestions only preselect the new-session picker)."
    } else if project.trusted {
        "Trusted: these exact bytes are in effect on top of your global defaults."
    } else {
        "Nothing to trust: no repository config. Global defaults apply."
    };
    let loaded = &project.project;
    let config = match loaded.path.as_deref().filter(|path| !path.is_empty()) {
        Some(path) if loaded.exists => {
            // JSON.stringify(project.config, null, 2), in the file's key order.
            let json = loaded
                .config
                .to_value()
                .and_then(|value| serde_json::to_string_pretty(&value))
                .unwrap_or_default();
            format!("{path}\n\n{json}")
        }
        Some(path) => format!("{path}\n\n(no deckhand.json in the main checkout)"),
        None => "Bare repository: no main checkout, so no repository deckhand.json.".to_owned(),
    };
    let purpose = purpose
        .filter(|purpose| !purpose.is_empty())
        .map(|purpose| format!("{purpose}\n\n"))
        .unwrap_or_default();
    format!(
        "{purpose}{status}\n\nTrusting lets this repository's commands (setup, Dev, actions) execute as you. Changed bytes need another review, except edits you save in Deckhand while it is trusted.\n\n{config}\n\nTrust covers these bytes, not everything commands may execute."
    )
}

/// TS: detailTexts.ts cleanupOverrideText (the typed-DELETE screen of x on a session).
pub fn cleanup_override_text(inspection: Option<&CleanupInspection>) -> String {
    let reasons = match inspection {
        Some(inspection) if !inspection.reasons.is_empty() => inspection.reasons.join("\n"),
        _ => "Safety could not be verified".to_owned(),
    };
    let files = inspection
        .and_then(|inspection| inspection.files.as_deref())
        .unwrap_or_default();
    let more = inspection.map_or(0, |inspection| {
        lost_file_count(inspection) - i64::try_from(files.len()).unwrap_or(i64::MAX)
    });
    let list = if files.is_empty() {
        String::new()
    } else {
        let lines: Vec<String> = files
            .iter()
            .map(|file| format!("  {} {}", cleanup_file_mark(file.state), file.path))
            .collect();
        let more = if more > 0 {
            format!("\n  +{more} more")
        } else {
            String::new()
        };
        format!("\n\nFiles ({CLEANUP_FILE_LEGEND}):\n{}{more}", lines.join("\n"))
    };
    format!(
        "This can permanently erase local work and commits.\n\n{reasons}{list}\n\nMain/current/shared worktree protections cannot be overridden."
    )
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
    let Some(project) = project else {
        return Vec::new();
    };
    let empty = IndexMap::new();
    let effective = project.effective.actions.as_ref().unwrap_or(&empty);
    let pending = if project.needs_review {
        project.project.config.actions.as_ref().unwrap_or(&empty)
    } else {
        &empty
    };
    let mut names: Vec<&String> = effective.keys().collect();
    names.extend(pending.keys().filter(|name| !effective.contains_key(*name)));
    names
        .into_iter()
        .map(|name| match pending.get(name) {
            Some(command) => ProjectAction {
                name: name.clone(),
                command: command.clone(),
                needs_trust: true,
                fallback: effective.get(name).cloned(),
            },
            None => ProjectAction {
                name: name.clone(),
                command: effective.get(name).cloned().unwrap_or_default(),
                needs_trust: false,
                fallback: None,
            },
        })
        .collect()
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
    let Some(summary) = summary else {
        return "Loading workspace information…".to_owned();
    };
    let pr = if options.creating_pr {
        format!(
            "Pushing {} to {} and opening GitHub…",
            summary.branch,
            summary.push_remote.as_deref().unwrap_or("its remote")
        )
    } else if pr_loading {
        "Requesting PR/check status…".to_owned()
    } else if let Some(pr) = &summary.pr {
        format!("PR #{}: {}, checks {}\n{}", pr.number, pr.state, pr.checks, pr.url)
    } else {
        summary
            .pr_error
            .clone()
            .unwrap_or_else(|| "PR lookup is opt-in: press P".to_owned())
    };
    let or_unknown =
        |value: Option<i64>, fallback: &str| value.map_or_else(|| fallback.to_owned(), |value| value.to_string());
    let mut lines = vec![
        format!("Workspace: {}", summary.cwd),
        format!("Branch: {}", summary.branch),
        format!(
            "HEAD: {}",
            if summary.head.is_empty() {
                "(no commits)"
            } else {
                &summary.head
            }
        ),
        format!("Base: {}", summary.base_ref.as_deref().unwrap_or("unknown")),
        String::new(),
        format!(
            "Uncommitted: {} files, +{} -{}",
            summary.changed_files, summary.additions, summary.deletions
        ),
        format!("Untracked: {} files", summary.untracked_files),
        format!(
            "Committed changes beyond base: {}",
            or_unknown(summary.commits_ahead_of_base, "unknown")
        ),
        format!(
            "Locally cached upstream: ahead {}, behind {}",
            or_unknown(summary.ahead, "?"),
            or_unknown(summary.behind, "?")
        ),
    ];
    if let Some(links) = &options.links {
        let skipped = if links.notes.is_empty() {
            String::new()
        } else {
            format!("; {} skipped:", links.notes.len())
        };
        lines.push(String::new());
        lines.push(format!("Worktree links: {} created{skipped}", links.linked.len()));
        lines.extend(links.notes.iter().map(|note| format!("  {note}")));
    }
    lines.extend([
        String::new(),
        pr,
        String::new(),
        "Changes belong to this workspace, not necessarily this agent. No automatic fetch; c pushes only after confirmation."
            .to_owned(),
    ]);
    lines.join("\n")
}

/// TS: detailTexts.ts createPrConfirmText.
pub fn create_pr_confirm_text(summary: &WorkspaceSummary) -> String {
    let remote = summary
        .push_remote
        .as_deref()
        .unwrap_or("(no remote: the push will fail)");
    let branch = &summary.branch;
    [
        format!("Push {branch} to {remote} and open GitHub's new-PR form in your browser?"),
        String::new(),
        format!("Runs: git push -u {remote} {branch}  (never forced)"),
        "Then: gh pr create --web (or gh pr view --web when a PR is already open).".to_owned(),
        String::new(),
        "Pushing publishes the branch's commits to the remote. Uncommitted changes are not pushed.".to_owned(),
        format!(
            "Uncommitted: {} files · untracked: {} files",
            summary.changed_files, summary.untracked_files
        ),
    ]
    .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    use deckhand_testkit::{Case, replay, tags};
    use serde_json::Value;

    /// A recorded `Pick<CleanupInspection, ...>`: `safe` is not part of the pick (and not read).
    fn inspection_arg(case: &Case) -> Result<Option<CleanupInspection>, String> {
        if case.arg_is_undefined(0) {
            return Ok(None);
        }
        let mut value = tags::untag(case.arg(0));
        if let Value::Object(object) = &mut value {
            object.entry("safe").or_insert(Value::Bool(false));
        }
        serde_json::from_value(value)
            .map(Some)
            .map_err(|error| error.to_string())
    }

    #[test]
    fn trust_review_text_parity() {
        replay("detailTexts/trustReviewText", |case: &Case| -> Result<String, String> {
            let purpose = if case.arg_is_undefined(1) {
                None
            } else {
                Some(case.arg_str(1)?)
            };
            Ok(trust_review_text(&case.arg_as(0)?, purpose))
        });
    }

    #[test]
    fn cleanup_override_text_parity() {
        replay(
            "detailTexts/cleanupOverrideText",
            |case: &Case| -> Result<String, String> { Ok(cleanup_override_text(inspection_arg(case)?.as_ref())) },
        );
    }

    #[test]
    fn project_actions_parity() {
        replay(
            "detailTexts/projectActions",
            |case: &Case| -> Result<Vec<ProjectAction>, String> {
                let project: Option<ProjectInfo> = if case.arg_is_undefined(0) {
                    None
                } else {
                    Some(case.arg_as(0)?)
                };
                Ok(project_actions(project.as_ref()))
            },
        );
    }

    #[test]
    fn workspace_summary_text_parity() {
        replay(
            "detailTexts/workspaceSummaryText",
            |case: &Case| -> Result<String, String> {
                let summary: Option<WorkspaceSummary> = if case.arg_is_undefined(0) {
                    None
                } else {
                    Some(case.arg_as(0)?)
                };
                let pr_loading = if case.arg_is_undefined(1) {
                    false
                } else {
                    case.arg_bool(1)?
                };
                let options = if case.arg_is_undefined(2) {
                    WorkspaceSummaryOptions::default()
                } else {
                    let value = tags::untag(case.arg(2));
                    WorkspaceSummaryOptions {
                        creating_pr: value.get("creatingPr").and_then(Value::as_bool).unwrap_or(false),
                        links: value
                            .get("links")
                            .map(|links| serde_json::from_value(links.clone()))
                            .transpose()
                            .map_err(|error| error.to_string())?,
                    }
                };
                Ok(workspace_summary_text(summary.as_ref(), pr_loading, &options))
            },
        );
    }

    #[test]
    fn create_pr_confirm_text_parity() {
        replay(
            "detailTexts/createPrConfirmText",
            |case: &Case| -> Result<String, String> { Ok(create_pr_confirm_text(&case.arg_as(0)?)) },
        );
    }
}
