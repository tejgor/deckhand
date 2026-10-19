//! Port of workspaceGit.ts.
//!
//! Read-mostly Git views of a workspace (a session's worktree or checkout): the summary shown with `i`, the cleanup
//! inspection a deletion is checked against, a handoff's Git context, the merge preview, merged-elsewhere detection
//! and worktree activity, plus pushing a branch and opening its PR with `gh`. Every Git run is bounded
//! (`deckhand_sys::git`); status is porcelain v2 with `--no-optional-locks`, so polling never takes the index lock.
//!
//! Data types are shared: the wire records are deckhand-protocol's ([`WorkspaceSummary`], [`PullRequestInfo`],
//! [`CleanupInspection`], [`CreatePrResult`], [`MergePreview`]); [`StatusEntry`] lives in
//! `deckhand_core::changes_model` and [`HandoffGitContext`] in `deckhand_core::session_features` (pure code consumes
//! them).

use std::rc::Rc;

use futures::future::LocalBoxFuture;
use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;

use deckhand_core::changes_model::StatusEntry;
use deckhand_core::error::Result;
use deckhand_core::session_features::{HandoffChange, HandoffDiffName, HandoffGitContext};
use deckhand_protocol::data::{CleanupInspection, CreatePrResult, MergePreview, PullRequestInfo, WorkspaceSummary};

/// TS: workspaceGit.ts CLEANUP_FILES_LISTED (files named by a cleanup inspection at most; the counts are complete).
pub const CLEANUP_FILES_LISTED: usize = 100;
/// TS: workspaceGit.ts MAX_SYMLINK_CHECKS (lstat calls per inspection; past it entries count as real data).
pub const MAX_SYMLINK_CHECKS: usize = 2000;
/// TS: workspaceGit.ts MAX_HANDOFF_COMMITS.
pub const MAX_HANDOFF_COMMITS: usize = 30;
/// TS: workspaceGit.ts MAX_HANDOFF_CHANGES.
pub const MAX_HANDOFF_CHANGES: usize = 50;
/// TS: workspaceGit.ts MERGE_PREVIEW_COMMITS (commit subjects shown by a merge preview; the count is exact).
pub const MERGE_PREVIEW_COMMITS: usize = 6;
/// TS: workspaceGit.ts MAX_OVERLAP (overlapping paths listed per kind).
pub const MAX_OVERLAP: usize = 200;
/// TS: workspaceGit.ts NON_INTERACTIVE (git push and gh never prompt).
pub const NON_INTERACTIVE: [(&str, &str); 3] = [
    ("GIT_TERMINAL_PROMPT", "0"),
    ("GH_PROMPT_DISABLED", "1"),
    ("GCM_INTERACTIVE", "never"),
];

/// TS: workspaceGit.ts PullRequestLookup: the branch's PR in a directory (`gh pr view`); injectable so tests never
/// reach GitHub. A failure with code `ENOENT` means gh is not installed.
pub type PullRequestLookup = Rc<dyn Fn(String) -> LocalBoxFuture<'static, Result<PullRequestInfo>>>;

/// TS: workspaceGit.ts WorkspaceStatus (parsed `git status --porcelain=v2 -z --branch`).
#[skip_serializing_none]
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceStatus {
    /// HEAD commit; `None` when unborn.
    pub oid: Option<String>,
    /// Branch name; `None` when detached.
    pub branch: Option<String>,
    pub upstream: Option<String>,
    pub ahead: Option<i64>,
    pub behind: Option<i64>,
    pub dirty_files: i64,
    pub untracked: Vec<String>,
    pub ignored: Vec<String>,
    /// Changed (`1`/`2`), unmerged (`u`) and untracked (`?`) entries in Git's order, for the Changes view.
    pub entries: Vec<StatusEntry>,
}

/// TS: workspaceGit.ts validateRef (`Invalid Git comparison ref`: empty, leading `-`, NUL or newlines).
pub fn validate_ref(reference: &str) -> Result<&str> {
    let _ = reference;
    todo!("I4")
}

/// TS: workspaceGit.ts base (the requested ref, else origin/HEAD, when it names a commit).
pub async fn base(cwd: &str, requested: Option<&str>) -> Option<String> {
    let _ = (cwd, requested);
    todo!("I4")
}

/// TS: workspaceGit.ts parseStatus (paths are repository-relative and may contain spaces or newlines).
pub fn parse_status(raw: &str) -> WorkspaceStatus {
    let _ = raw;
    todo!("I4")
}

/// TS: workspaceGit.ts readWorkspaceStatus (`include_ignored`: `--ignored=matching`, for cleanup inspection).
pub async fn read_workspace_status(cwd: &str, include_ignored: bool) -> Result<WorkspaceStatus> {
    let _ = (cwd, include_ignored);
    todo!("I4")
}

/// TS: workspaceGit.ts numstat result (line totals of a `--numstat -z` diff; binary files count 0).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NumstatTotals {
    pub additions: i64,
    pub deletions: i64,
}

/// TS: workspaceGit.ts numstat.
pub fn numstat(raw: &str) -> NumstatTotals {
    let _ = raw;
    todo!("I4")
}

/// TS: workspaceGit.ts getWorkspaceSummary (`include_pr`: look the PR up with `lookup_pr`, default
/// [`gh_pull_request`]; a lookup failure becomes `prError`).
pub async fn get_workspace_summary(
    cwd: &str,
    base_ref: Option<&str>,
    include_pr: bool,
    lookup_pr: Option<PullRequestLookup>,
) -> Result<WorkspaceSummary> {
    let _ = (cwd, base_ref, include_pr, lookup_pr);
    todo!("I4")
}

/// TS: workspaceGit.ts ghPullRequest (`gh pr view --json ...` in `cwd`, bounded; fails when gh is missing (code
/// `ENOENT`), unauthenticated or finds none, or with `Invalid PR response`).
pub async fn gh_pull_request(cwd: &str) -> Result<PullRequestInfo> {
    let _ = cwd;
    todo!("I4")
}

/// TS: workspaceGit.ts disposableIgnored (ignored files under `node_modules` are never worth keeping).
pub fn disposable_ignored(file: &str) -> bool {
    let _ = file;
    todo!("I4")
}

/// TS: workspaceGit.ts inspectWorkspaceCleanup options.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CleanupOptions {
    /// The branch is deleted too: commits only on it count as lost.
    pub delete_branch: bool,
    /// A squash-merged commit (40-64 hex): it and its ancestors are integrated.
    pub integrated: Option<String>,
}

/// TS: workspaceGit.ts inspectWorkspaceCleanup (what removing the worktree would lose: uncommitted, untracked and
/// valuable ignored files, symlinks excepted; commits only when the branch goes too or HEAD is detached). A failed
/// status read is an unsafe inspection, not an error.
pub async fn inspect_workspace_cleanup(
    cwd: &str,
    base_ref: Option<&str>,
    options: &CleanupOptions,
) -> Result<CleanupInspection> {
    let _ = (cwd, base_ref, options);
    todo!("I4")
}

/// TS: workspaceGit.ts pushRemote (the branch's configured upstream remote, else `origin` when it exists).
pub async fn push_remote(cwd: &str, branch: &str) -> Option<String> {
    let _ = (cwd, branch);
    todo!("I4")
}

/// TS: workspaceGit.ts createPullRequest options.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CreatePrOptions {
    pub base_ref: Option<String>,
    /// The branch the UI showed: a different current branch is refused.
    pub expected_branch: Option<String>,
}

/// TS: workspaceGit.ts gh (gh in `cwd`, stdin closed, prompts disabled, `timeout` default 60 s).
pub async fn gh(
    cwd: &str,
    args: &[&str],
    timeout: Option<std::time::Duration>,
) -> std::result::Result<crate::proc::ExecOutput, crate::proc::ExecError> {
    let _ = (cwd, args, timeout);
    todo!("I4")
}

/// TS: workspaceGit.ts firstLine (an execFile error's stderr, else its message: the last three non-empty lines
/// joined, at most 400 characters).
pub fn first_line(error: &crate::proc::ExecError) -> String {
    let _ = error;
    todo!("I4")
}

/// TS: workspaceGit.ts createPullRequest (`git push -u <remote> <branch>`, never forced, then `gh pr create --web` or
/// `gh pr view --web` for an open PR; refuses detached HEAD, main/master and the session's base branch).
pub async fn create_pull_request(cwd: &str, options: &CreatePrOptions) -> Result<CreatePrResult> {
    let _ = (cwd, options);
    todo!("I4")
}

/// TS: workspaceGit.ts changedFiles (porcelain v1 -z: `XY path`, renames/copies followed by their original path).
pub fn changed_files(raw: &str) -> Vec<HandoffChange> {
    let _ = raw;
    todo!("I4")
}

/// TS: workspaceGit.ts numstatFiles (`a → b` for renames; no counts for binary files).
pub fn numstat_files(raw: &str) -> Vec<HandoffDiffName> {
    let _ = raw;
    todo!("I4")
}

/// TS: workspaceGit.ts getHandoffGitContext (`None` outside a Git repository, unless `cwd` is gone, which is an
/// `error`; other failures are returned as `error` so the export still succeeds).
pub async fn get_handoff_git_context(cwd: &str, base_ref: Option<&str>) -> Option<HandoffGitContext> {
    let _ = (cwd, base_ref);
    todo!("I4")
}

/// TS: workspaceGit.ts numstatPaths (every path a numstat -z output names, both sides of a rename).
pub fn numstat_paths(raw: &str) -> Vec<String> {
    let _ = raw;
    todo!("I4")
}

/// TS: workspaceGit.ts statusPaths (every entry's path and original path).
pub fn status_paths(status: &WorkspaceStatus) -> Vec<String> {
    let _ = status;
    todo!("I4")
}

/// TS: workspaceGit.ts getMergePreview (read-only, bounded: commits in `target..source`, the committed diff stat,
/// the source's uncommitted files, an operation in progress in the target, overlapping uncommitted files).
pub async fn get_merge_preview(source_cwd: &str, target_cwd: &str) -> Result<MergePreview> {
    let _ = (source_cwd, target_cwd);
    todo!("I4")
}

/// TS: workspaceGit.ts AncestryCheck.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AncestryCheck {
    /// Any worktree of the repository.
    pub cwd: String,
    /// The branch's tip.
    pub tip: String,
    /// The branch's name (empty when detached).
    pub branch: String,
    /// The commit the branch started from.
    pub start: String,
    /// Fully qualified refs of the default branch, local and `origin/` (whichever exist).
    pub defaults: Vec<String>,
}

/// TS: workspaceGit.ts mergedIntoDefault (the tip is reachable from the default branch and has a commit of its own
/// beyond its start).
pub async fn merged_into_default(check: &AncestryCheck) -> bool {
    let _ = check;
    todo!("I4")
}

/// TS: workspaceGit.ts worktreeActivity result (each absent when Git cannot tell).
#[skip_serializing_none]
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeActivity {
    pub last_commit_at: Option<String>,
    pub ahead_of_default: Option<i64>,
}

/// TS: workspaceGit.ts worktreeActivity (when HEAD was last committed, and its commits none of `defaults` has).
pub async fn worktree_activity(cwd: &str, defaults: &[String]) -> WorktreeActivity {
    let _ = (cwd, defaults);
    todo!("I4")
}
