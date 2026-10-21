//! Read-mostly Git views of a workspace (a session's worktree or checkout): the summary shown with `i`, the cleanup
//! inspection a deletion is checked against, a handoff's Git context, the merge preview, merged-elsewhere detection
//! and worktree activity, plus pushing a branch and opening its PR with `gh`. Every Git run is bounded
//! (`deckhand_sys::git`); status is porcelain v2 with `--no-optional-locks`, so polling never takes the index lock.
//!
//! Data types are shared: the wire records are deckhand-protocol's ([`WorkspaceSummary`], [`PullRequestInfo`],
//! [`CleanupInspection`], [`CreatePrResult`], [`MergePreview`]); [`StatusEntry`] lives in
//! `deckhand_core::changes_model` and [`HandoffGitContext`] in `deckhand_core::session_features` (pure code consumes
//! them).

use std::collections::HashSet;
use std::rc::Rc;
use std::time::Duration;

use futures::future::LocalBoxFuture;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use serde_with::skip_serializing_none;

use crate::git::{
    BranchWhere, GitOptions, branch_has_own_commits, current_branch, find_repo_root, git, operation_in_progress,
    optional_git, resolve_default_branch, resolve_repo_context,
};
use crate::proc::{self, ExecError, ExecOptions};
use deckhand_core::changes_model::{StatusEntry, StatusKind};
use deckhand_core::error::{DhError, Result};
use deckhand_core::jsstr::{self, path};
use deckhand_core::session_features::{HandoffChange, HandoffDiff, HandoffDiffName, HandoffGitContext};
use deckhand_protocol::data::{
    CleanupFile, CleanupFileState, CleanupInspection, CreatePrResult, DiffStat, JsNumber, MergeOverlap, MergePreview,
    PrChecks, PullRequestInfo, WorkspaceSummary,
};

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

/// `Number(text)` for Git's count outputs (trimmed; empty is 0; anything else unparsable NaN).
fn js_number(text: &str) -> f64 {
    let text = jsstr::js_trim(text);
    if text.is_empty() {
        return 0.0;
    }
    if !text
        .bytes()
        .all(|byte| byte.is_ascii_digit() || matches!(byte, b'.' | b'-' | b'+' | b'e' | b'E'))
    {
        return f64::NAN;
    }
    text.parse::<f64>().unwrap_or(f64::NAN)
}

/// `Number(text) || 0` as an integer count.
#[allow(clippy::cast_possible_truncation)]
fn count_or_zero(text: &str) -> i64 {
    let value = js_number(text);
    if value.is_nan() { 0 } else { value as i64 }
}

/// A `\d+` run as a JS number (integer here; digit runs past i64 saturate).
#[allow(clippy::cast_possible_truncation)]
fn digits_number(text: &str) -> i64 {
    text.parse::<f64>().map_or(0, |value| value as i64)
}

/// `/^[0-9a-f]{40,64}$/`.
fn is_sha(text: &str) -> bool {
    (40..=64).contains(&text.len()) && text.bytes().all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

/// A numstat count field: `\d+` or `-`.
fn is_count_field(text: &str) -> bool {
    text == "-" || (!text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit()))
}

/// `record.match(/^(\d+|-)\t(\d+|-)\t([\s\S]*)$/)`: added, deleted and the path (empty for a rename).
fn numstat_record(record: &str) -> Option<(&str, &str, &str)> {
    let (added, rest) = record.split_once('\t')?;
    let (deleted, file) = rest.split_once('\t')?;
    (is_count_field(added) && is_count_field(deleted)).then_some((added, deleted, file))
}

/// TS: workspaceGit.ts validateRef (`Invalid Git comparison ref`: empty, leading `-`, NUL or newlines).
pub fn validate_ref(reference: &str) -> Result<&str> {
    if reference.is_empty() || reference.starts_with('-') || reference.contains(['\0', '\r', '\n']) {
        return Err(DhError::msg("Invalid Git comparison ref"));
    }
    Ok(reference)
}

/// TS: workspaceGit.ts base (the requested ref, else origin/HEAD, when it names a commit).
pub async fn base(cwd: &str, requested: Option<&str>) -> Option<String> {
    let reference = match requested.filter(|requested| !requested.is_empty()) {
        Some(requested) => requested.to_owned(),
        None => optional_git(cwd, &["symbolic-ref", "--quiet", "refs/remotes/origin/HEAD"], None).await?,
    };
    if reference.is_empty() {
        return None;
    }
    let commit = format!("{reference}^{{commit}}");
    let verified = optional_git(cwd, &["rev-parse", "--verify", "--quiet", &commit], None).await;
    verified.filter(|out| !out.is_empty()).map(|_| reference)
}

/// `# branch.ab +<ahead> -<behind>` (`/^\+(\d+) -(\d+)$/`).
fn parse_ab(value: &str) -> Option<(i64, i64)> {
    let (ahead, behind) = value.strip_prefix('+')?.split_once(" -")?;
    let digits = |text: &str| !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit());
    (digits(ahead) && digits(behind)).then(|| (digits_number(ahead), digits_number(behind)))
}

/// TS: workspaceGit.ts parseStatus (paths are repository-relative and may contain spaces or newlines).
pub fn parse_status(raw: &str) -> WorkspaceStatus {
    let mut status = WorkspaceStatus::default();
    let records: Vec<&str> = raw.split('\0').collect();
    let mut index = 0;
    while let Some(&record) = records.get(index) {
        match record.as_bytes().first() {
            Some(b'#') => {
                // `const [, key, ...rest] = record.split(' ')`: the value keeps its spaces.
                let mut parts = record.splitn(3, ' ');
                parts.next();
                let key = parts.next();
                let value = parts.next().unwrap_or("");
                match key {
                    Some("branch.oid") => status.oid = (value != "(initial)").then(|| value.to_owned()),
                    Some("branch.head") => status.branch = (value != "(detached)").then(|| value.to_owned()),
                    Some("branch.upstream") => status.upstream = Some(value.to_owned()),
                    Some("branch.ab") => {
                        if let Some((ahead, behind)) = parse_ab(value) {
                            status.ahead = Some(ahead);
                            status.behind = Some(behind);
                        }
                    }
                    _ => {}
                }
            }
            Some(&kind @ (b'1' | b'2' | b'u')) => {
                status.dirty_files += 1;
                let fields: Vec<&str> = record.split(' ').collect();
                // Space-separated fields before the path (the path itself may contain spaces).
                let (kind, path_field) = match kind {
                    b'1' => (StatusKind::Ordinary, 8),
                    b'2' => (StatusKind::Renamed, 9),
                    _ => (StatusKind::Unmerged, 10),
                };
                let mut entry = StatusEntry {
                    kind,
                    xy: fields.get(1).copied().unwrap_or("..").to_owned(),
                    path: fields.get(path_field..).map(|rest| rest.join(" ")).unwrap_or_default(),
                    orig_path: None,
                };
                // Rename/copy: the original path is the next NUL-separated field.
                if kind == StatusKind::Renamed {
                    index += 1;
                    entry.orig_path = Some(records.get(index).copied().unwrap_or("").to_owned());
                }
                status.entries.push(entry);
            }
            Some(b'?') => {
                let file = jsstr::js_slice(record, 2, None);
                status.untracked.push(file.clone());
                status.entries.push(StatusEntry {
                    kind: StatusKind::Untracked,
                    xy: "??".to_owned(),
                    path: file,
                    orig_path: None,
                });
            }
            Some(b'!') => status.ignored.push(jsstr::js_slice(record, 2, None)),
            _ => {}
        }
        index += 1;
    }
    status
}

/// TS: workspaceGit.ts readWorkspaceStatus (`include_ignored`: `--ignored=matching`, for cleanup inspection).
pub async fn read_workspace_status(cwd: &str, include_ignored: bool) -> Result<WorkspaceStatus> {
    // Cleanup inspection needs ignored files (--ignored=matching collapses ignored directories to one entry instead
    // of listing every file); the summary skips that scan.
    let mut args = vec![
        "--no-optional-locks",
        "status",
        "--porcelain=v2",
        "-z",
        "--branch",
        "--untracked-files=all",
    ];
    if include_ignored {
        args.push("--ignored=matching");
    }
    let options = GitOptions::default().with_max_buffer(32 * 1024 * 1024);
    Ok(parse_status(&git(cwd, &args, Some(&options)).await?))
}

/// TS: workspaceGit.ts numstat result (line totals of a `--numstat -z` diff; binary files count 0).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NumstatTotals {
    pub additions: i64,
    pub deletions: i64,
}

/// TS: workspaceGit.ts numstat.
pub fn numstat(raw: &str) -> NumstatTotals {
    let mut totals = NumstatTotals::default();
    let records: Vec<&str> = raw.split('\0').collect();
    let mut index = 0;
    while let Some(record) = records.get(index) {
        if let Some((added, deleted, file)) = numstat_record(record) {
            // `Number('-') || 0`.
            totals.additions = totals
                .additions
                .saturating_add(if added == "-" { 0 } else { digits_number(added) });
            totals.deletions = totals
                .deletions
                .saturating_add(if deleted == "-" { 0 } else { digits_number(deleted) });
            // Rename: empty path then old and new paths.
            if file.is_empty() {
                index += 2;
            }
        }
        index += 1;
    }
    totals
}

/// The default [`PullRequestLookup`]: [`gh_pull_request`].
fn default_lookup() -> PullRequestLookup {
    Rc::new(|cwd: String| Box::pin(async move { gh_pull_request(&cwd).await }))
}

/// TS: workspaceGit.ts getWorkspaceSummary (`include_pr`: look the PR up with `lookup_pr`, default
/// [`gh_pull_request`]; a lookup failure becomes `prError`).
pub async fn get_workspace_summary(
    cwd: &str,
    base_ref: Option<&str>,
    include_pr: bool,
    lookup_pr: Option<PullRequestLookup>,
) -> Result<WorkspaceSummary> {
    let base_ref = base_ref.filter(|reference| !reference.is_empty());
    if let Some(reference) = base_ref {
        validate_ref(reference)?;
    }
    let status = read_workspace_status(cwd, false).await?;
    let head = status.oid.clone().unwrap_or_default();
    let diff_args: Vec<&str> = if head.is_empty() {
        vec!["--no-optional-locks", "diff", "--cached", "--numstat", "-z", "--"]
    } else {
        vec!["--no-optional-locks", "diff", "HEAD", "--numstat", "-z", "--"]
    };
    let (resolved_base, diff) = futures::join!(base(cwd, base_ref), git(cwd, &diff_args, None));
    let totals = numstat(&diff?);
    let mut result = WorkspaceSummary {
        cwd: cwd.to_owned(),
        branch: status.branch.clone().unwrap_or_else(|| "(detached)".to_owned()),
        head: head.clone(),
        base_ref: resolved_base.clone(),
        changed_files: status.dirty_files,
        untracked_files: i64::try_from(status.untracked.len()).unwrap_or(i64::MAX),
        additions: totals.additions,
        deletions: totals.deletions,
        ahead: None,
        behind: None,
        pr: None,
        pr_error: None,
        commits_ahead_of_base: None,
        push_remote: None,
    };
    if status.upstream.is_some() && status.ahead.is_some() {
        result.ahead = status.ahead;
        result.behind = status.behind;
    }
    let ahead_of_base = async {
        match &resolved_base {
            Some(resolved) if !head.is_empty() => {
                let range = format!("{resolved}..HEAD");
                Some(git(cwd, &["rev-list", "--count", &range], None).await)
            }
            _ => None,
        }
    };
    let remote = async {
        match &status.branch {
            Some(branch) => push_remote(cwd, branch).await,
            None => None,
        }
    };
    let pr = async {
        if !include_pr {
            return None;
        }
        let lookup = lookup_pr.clone().unwrap_or_else(default_lookup);
        Some(lookup(cwd.to_owned()).await)
    };
    let (ahead_of_base, remote, pr) = futures::join!(ahead_of_base, remote, pr);
    let ahead_of_base = ahead_of_base.transpose()?;
    match pr {
        Some(Ok(pr)) => result.pr = Some(pr),
        Some(Err(error)) => {
            result.pr_error = Some(if error.is_not_found() {
                "Install gh for PR status".to_owned()
            } else {
                "No PR status available; check gh authentication and branch".to_owned()
            });
        }
        None => {}
    }
    if let Some(count) = ahead_of_base {
        result.commits_ahead_of_base = Some(count_or_zero(&count));
    }
    if let Some(remote) = remote.filter(|remote| !remote.is_empty()) {
        result.push_remote = Some(remote);
    }
    Ok(result)
}

/// JS truthiness of a JSON value (`undefined` is `None`).
fn truthy(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => false,
        Some(Value::Bool(flag)) => *flag,
        Some(Value::Number(number)) => number.as_f64().is_some_and(|n| n != 0.0 && !n.is_nan()),
        Some(Value::String(text)) => !text.is_empty(),
        Some(Value::Array(_) | Value::Object(_)) => true,
    }
}

/// `value.<name>` on a parsed JSON value: a TypeError for `null`, `undefined` for primitives and arrays.
fn property<'a>(value: &'a Value, name: &str) -> Result<Option<&'a Value>> {
    match value {
        Value::Null => Err(DhError::msg(format!(
            "Cannot read properties of null (reading '{name}')"
        ))),
        Value::Object(map) => Ok(map.get(name)),
        _ => Ok(None),
    }
}

/// `check.<first> || check.<second>`, then `list.includes(..)`.
fn check_in(check: &Value, first: &str, second: &str, list: &[&str]) -> Result<bool> {
    let primary = property(check, first)?;
    let value = if truthy(primary) {
        primary
    } else {
        property(check, second)?
    };
    Ok(value.and_then(Value::as_str).is_some_and(|text| list.contains(&text)))
}

/// `String(value)` for a parsed JSON value.
fn js_string(value: Option<&Value>) -> String {
    match value {
        None => "undefined".to_owned(),
        Some(Value::Null) => "null".to_owned(),
        Some(Value::Bool(flag)) => flag.to_string(),
        Some(Value::Number(number)) => JsNumber(number.as_f64().unwrap_or(f64::NAN)).to_js_string(),
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| match item {
                Value::Null => String::new(),
                item => js_string(Some(item)),
            })
            .collect::<Vec<_>>()
            .join(","),
        Some(Value::Object(_)) => "[object Object]".to_owned(),
    }
}

/// The PR from `gh pr view --json` output (ghPullRequest after the run).
fn pull_request_from_json(stdout: &str) -> Result<PullRequestInfo> {
    let pr: Value = serde_json::from_str(stdout).map_err(|_| DhError::json_parse(stdout))?;
    let number = property(&pr, "number")?
        .and_then(Value::as_f64)
        .filter(|number| number.is_finite() && number.fract() == 0.0);
    let url = property(&pr, "url")?
        .and_then(Value::as_str)
        .filter(|url| url.starts_with("https://"));
    let (Some(number), Some(url)) = (number, url) else {
        return Err(DhError::msg("Invalid PR response"));
    };
    let empty = Vec::new();
    let checks = property(&pr, "statusCheckRollup")?
        .and_then(Value::as_array)
        .unwrap_or(&empty);
    const FAILED: [&str; 6] = [
        "FAILURE",
        "ERROR",
        "CANCELLED",
        "TIMED_OUT",
        "ACTION_REQUIRED",
        "STARTUP_FAILURE",
    ];
    const PENDING: [&str; 5] = ["PENDING", "QUEUED", "IN_PROGRESS", "WAITING", "REQUESTED"];
    const PASSED: [&str; 3] = ["SUCCESS", "NEUTRAL", "SKIPPED"];
    let mut failed = false;
    for check in checks {
        if check_in(check, "conclusion", "state", &FAILED)? {
            failed = true;
            break;
        }
    }
    let mut pending = false;
    for check in checks {
        if check_in(check, "state", "status", &PENDING)? {
            pending = true;
            break;
        }
    }
    let state = if failed {
        PrChecks::Failing
    } else if pending {
        PrChecks::Pending
    } else {
        let mut passing = !checks.is_empty();
        for check in checks {
            if !passing {
                break;
            }
            passing = check_in(check, "conclusion", "state", &PASSED)?;
        }
        if passing { PrChecks::Passing } else { PrChecks::Unknown }
    };
    let head_sha = property(&pr, "headRefOid")?
        .and_then(Value::as_str)
        .filter(|sha| is_sha(sha))
        .map(str::to_owned);
    let base_branch = property(&pr, "baseRefName")?
        .and_then(Value::as_str)
        .filter(|name| !name.is_empty() && !name.starts_with('-'))
        .map(str::to_owned);
    #[allow(clippy::cast_possible_truncation)]
    Ok(PullRequestInfo {
        number: number as i64,
        url: url.to_owned(),
        state: js_string(property(&pr, "state")?),
        checks: state,
        head_sha,
        base_branch,
    })
}

/// TS: workspaceGit.ts ghPullRequest (`gh pr view --json ...` in `cwd`, bounded; fails when gh is missing (code
/// `ENOENT`), unauthenticated or finds none, or with `Invalid PR response`).
pub async fn gh_pull_request(cwd: &str) -> Result<PullRequestInfo> {
    let options = ExecOptions::default()
        .cwd(cwd)
        .timeout(Duration::from_millis(8000))
        .max_buffer(1024 * 1024)
        .env("GH_PROMPT_DISABLED", "1");
    let output = proc::exec(
        "gh",
        &[
            "pr",
            "view",
            "--json",
            "number,url,state,statusCheckRollup,headRefOid,baseRefName",
        ],
        &options,
    )
    .await?;
    pull_request_from_json(&output.stdout)
}

/// TS: workspaceGit.ts disposableIgnored (ignored files under `node_modules` are never worth keeping).
pub fn disposable_ignored(file: &str) -> bool {
    file.split('/').any(|part| part == "node_modules")
}

/// TS: workspaceGit.ts symlinkCheck: lstat-based symlink test relative to the repository root; past the cap entries
/// count as real data (conservative).
struct SymlinkCheck {
    enabled: bool,
    top: Option<String>,
    checks: usize,
}

impl SymlinkCheck {
    async fn new(cwd: &str, entries: usize) -> Self {
        if entries == 0 {
            return SymlinkCheck {
                enabled: false,
                top: None,
                checks: 0,
            };
        }
        SymlinkCheck {
            enabled: true,
            top: find_repo_root(Some(cwd)).await.ok(),
            checks: 0,
        }
    }

    async fn is_link(&mut self, file: &str) -> bool {
        if !self.enabled {
            return false;
        }
        let Some(top) = self.top.as_deref() else {
            return false;
        };
        if file.ends_with('/') {
            return false;
        }
        self.checks += 1;
        if self.checks > MAX_SYMLINK_CHECKS {
            return false;
        }
        tokio::fs::symlink_metadata(path::join(&[top, file]))
            .await
            .is_ok_and(|stat| stat.file_type().is_symlink())
    }
}

/// TS: workspaceGit.ts inspectWorkspaceCleanup options.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CleanupOptions {
    /// The branch is deleted too: commits only on it count as lost.
    pub delete_branch: bool,
    /// A squash-merged commit (40-64 hex): it and its ancestors are integrated.
    pub integrated: Option<String>,
}

/// `error.message.split('\n')[0]`.
fn first_message_line(message: &str) -> &str {
    message.split('\n').next().unwrap_or("")
}

/// TS: workspaceGit.ts inspectWorkspaceCleanup (what removing the worktree would lose: uncommitted, untracked and
/// valuable ignored files, symlinks excepted; commits only when the branch goes too or HEAD is detached). A failed
/// status read is an unsafe inspection, not an error.
///
/// Removing a worktree loses its uncommitted/untracked/valuable ignored files. Commits are kept by the branch, so
/// they only matter when the branch is deleted too or HEAD is detached; then only commits reachable from no other
/// local or remote-tracking branch count. Upstream/base comparisons are informational.
pub async fn inspect_workspace_cleanup(
    cwd: &str,
    base_ref: Option<&str>,
    options: &CleanupOptions,
) -> Result<CleanupInspection> {
    let base_ref = base_ref.filter(|reference| !reference.is_empty());
    if let Some(reference) = base_ref {
        validate_ref(reference)?;
    }
    let status = match read_workspace_status(cwd, true).await {
        Ok(status) => status,
        Err(error) => {
            return Ok(CleanupInspection {
                safe: false,
                reasons: vec![format!(
                    "Git status unavailable: {}",
                    first_message_line(error.message())
                )],
                dirty_files: 0,
                untracked_files: 0,
                ignored_files: 0,
                files: None,
                unpublished_commits: None,
                unmerged_commits: None,
            });
        }
    };
    // Symlinks hold no data of their own (shared dependency dirs, linked env files): removing one never touches its
    // target, so they are disposable. Paths are checked with lstat, never followed.
    let mut links = SymlinkCheck::new(cwd, status.untracked.len() + status.ignored.len()).await;
    let mut untracked: Vec<String> = Vec::new();
    for file in &status.untracked {
        if !links.is_link(file).await {
            untracked.push(file.clone());
        }
    }
    let mut ignored: Vec<String> = Vec::new();
    for file in &status.ignored {
        if !disposable_ignored(file) && !links.is_link(file).await {
            ignored.push(file.clone());
        }
    }
    let mut reasons: Vec<String> = Vec::new();
    if status.dirty_files != 0 {
        reasons.push(format!("{} modified/staged file(s)", status.dirty_files));
    }
    if !untracked.is_empty() {
        reasons.push(format!("{} untracked file(s)", untracked.len()));
    }
    if !ignored.is_empty() {
        let first: Vec<&str> = ignored.iter().take(3).map(String::as_str).collect();
        reasons.push(format!(
            "{} valuable ignored file(s), including {}",
            ignored.len(),
            first.join(", ")
        ));
    }
    let has_oid = status.oid.as_ref().is_some_and(|oid| !oid.is_empty());
    let check_commits = has_oid && (options.delete_branch || status.branch.as_ref().is_none_or(String::is_empty));
    // --exclude patterns for --branches omit refs/heads/.
    let exclusive: Vec<String> = match status.branch.as_ref().filter(|branch| !branch.is_empty()) {
        Some(branch) => vec![format!("--exclude={branch}")],
        None => Vec::new(),
    };
    // A squash merge (Deckhand's or a PR's) leaves the branch unmerged as far as Git knows: the commit recorded at
    // merge time and its ancestors are integrated; commits made after it still count.
    let mut integrated: Vec<String> = Vec::new();
    if check_commits && let Some(commit) = options.integrated.as_ref().filter(|commit| is_sha(commit)) {
        let verify = format!("{commit}^{{commit}}");
        if optional_git(cwd, &["rev-parse", "--verify", "--quiet", &verify], None)
            .await
            .is_some_and(|out| !out.is_empty())
        {
            integrated.push(commit.clone());
        }
    }
    let merged = async {
        let resolved = base(cwd, base_ref).await?;
        if !has_oid {
            return None;
        }
        let range = format!("{resolved}..HEAD");
        optional_git(cwd, &["rev-list", "--count", &range], None).await
    };
    let lost = async {
        if !check_commits {
            return None;
        }
        let mut args: Vec<&str> = vec!["rev-list", "--count", "HEAD", "--not"];
        args.extend(exclusive.iter().map(String::as_str));
        args.extend(["--branches", "--remotes"]);
        args.extend(integrated.iter().map(String::as_str));
        optional_git(cwd, &args, None).await
    };
    let (merged, lost) = futures::join!(merged, lost);
    if check_commits {
        let place = match status.branch.as_ref().filter(|branch| !branch.is_empty()) {
            Some(branch) => format!("only on branch {branch}"),
            None => "only on the detached HEAD".to_owned(),
        };
        match lost {
            None => reasons.push("Commit preservation cannot be verified".to_owned()),
            Some(lost) => {
                let count = js_number(&lost);
                if count > 0.0 {
                    reasons.push(format!(
                        "{} commit(s) exist {place} and would be lost",
                        JsNumber(count).to_js_string()
                    ));
                }
            }
        }
    }
    let files: Vec<CleanupFile> = status
        .entries
        .iter()
        .filter(|entry| entry.kind != StatusKind::Untracked)
        .map(|entry| CleanupFile {
            path: entry.path.clone(),
            state: CleanupFileState::Changed,
        })
        .chain(untracked.iter().map(|file| CleanupFile {
            path: file.clone(),
            state: CleanupFileState::Untracked,
        }))
        .chain(ignored.iter().map(|file| CleanupFile {
            path: file.clone(),
            state: CleanupFileState::Ignored,
        }))
        .take(CLEANUP_FILES_LISTED)
        .collect();
    #[allow(clippy::cast_possible_truncation)]
    Ok(CleanupInspection {
        safe: reasons.is_empty(),
        reasons,
        dirty_files: status.dirty_files,
        untracked_files: i64::try_from(untracked.len()).unwrap_or(i64::MAX),
        ignored_files: i64::try_from(ignored.len()).unwrap_or(i64::MAX),
        files: (!files.is_empty()).then_some(files),
        unpublished_commits: if status.upstream.as_ref().is_some_and(|upstream| !upstream.is_empty()) {
            status.ahead
        } else {
            None
        },
        unmerged_commits: merged.map(|merged| js_number(&merged) as i64),
    })
}

/// TS: workspaceGit.ts pushRemote (the branch's configured upstream remote, else `origin` when it exists).
pub async fn push_remote(cwd: &str, branch: &str) -> Option<String> {
    let key = format!("branch.{branch}.remote");
    let configured = optional_git(cwd, &["config", "--get", &key], None).await;
    if let Some(configured) =
        configured.filter(|remote| !remote.is_empty() && remote != "." && !remote.starts_with('-'))
    {
        return Some(configured);
    }
    let remotes = optional_git(cwd, &["remote"], None).await?;
    remotes
        .split('\n')
        .any(|remote| remote == "origin")
        .then(|| "origin".to_owned())
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
    let mut options = ExecOptions::default()
        .cwd(cwd)
        .timeout(timeout.unwrap_or(Duration::from_millis(60_000)))
        .max_buffer(1024 * 1024)
        // An empty stdin, ended at once (TS `child.stdin?.end()`).
        .stdin(Vec::new());
    for (key, value) in NON_INTERACTIVE {
        options = options.env(key, value);
    }
    proc::exec("gh", args, &options).await
}

/// TS firstLine on any error: `(stderr?.trim() || message)`, its last three non-empty trimmed lines joined by spaces,
/// at most 400 characters.
fn first_line_of(stderr: Option<&str>, message: &str) -> String {
    let source = stderr
        .map(jsstr::js_trim)
        .filter(|text| !text.is_empty())
        .unwrap_or(message);
    let lines: Vec<&str> = source
        .split('\n')
        .map(jsstr::js_trim)
        .filter(|line| !line.is_empty())
        .collect();
    let tail = &lines[lines.len().saturating_sub(3)..];
    jsstr::js_slice(&tail.join(" "), 0, Some(400))
}

/// TS: workspaceGit.ts firstLine (an execFile error's stderr, else its message: the last three non-empty lines
/// joined, at most 400 characters).
pub fn first_line(error: &crate::proc::ExecError) -> String {
    first_line_of(Some(&error.stderr), &error.message())
}

/// A gh failure: not installed, or its output.
fn gh_error(branch: &str, remote: &str, code_enoent: bool, detail: &str) -> DhError {
    if code_enoent {
        DhError::msg(format!(
            "Pushed {branch} to {remote}, but gh is not installed; install GitHub CLI to open the PR form"
        ))
    } else {
        DhError::msg(format!("Pushed {branch} to {remote}, but gh failed: {detail}"))
    }
}

/// `/no (open )?pull requests? found/i`.
fn no_pull_request_found(text: &str) -> bool {
    let lower = text.to_lowercase();
    [
        "no pull request found",
        "no pull requests found",
        "no open pull request found",
        "no open pull requests found",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

/// TS: workspaceGit.ts createPullRequest (`git push -u <remote> <branch>`, never forced, then `gh pr create --web` or
/// `gh pr view --web` for an open PR; refuses detached HEAD, main/master and the session's base branch).
pub async fn create_pull_request(cwd: &str, options: &CreatePrOptions) -> Result<CreatePrResult> {
    if let Some(reference) = options.base_ref.as_deref().filter(|reference| !reference.is_empty()) {
        validate_ref(reference)?;
    }
    let branch = git(cwd, &["branch", "--show-current"], None).await?;
    if branch.is_empty() {
        return Err(DhError::msg(
            "HEAD is detached; check out a branch before creating a PR",
        ));
    }
    if let Some(expected) = &options.expected_branch
        && *expected != branch
    {
        return Err(DhError::msg(format!(
            "Branch changed to {branch}; reopen i before creating a PR"
        )));
    }
    let Some(remote) = push_remote(cwd, &branch).await else {
        return Err(DhError::msg(format!(
            "No remote to push {branch} to: set an upstream or add an \"origin\" remote"
        )));
    };
    let prefix = format!("{remote}/");
    let base_name: Option<String> = options
        .base_ref
        .as_ref()
        .map(|reference| match reference.strip_prefix(&prefix) {
            Some(rest) => rest.to_owned(),
            None => reference.clone(),
        });
    let is_base = base_name.as_deref() == Some(branch.as_str());
    if branch == "main" || branch == "master" || is_base {
        return Err(DhError::msg(format!(
            "Refusing to create a PR from {branch}: it is {}",
            if is_base {
                "the session's base branch"
            } else {
                "a protected branch"
            }
        )));
    }
    // --base only when the base exists on the remote as a branch (e.g. origin/main, or a launch branch that was
    // pushed); otherwise gh chooses.
    let mut base = None;
    if let Some(name) = base_name
        .as_ref()
        .filter(|name| !name.is_empty() && !name.starts_with('-'))
    {
        let reference = format!("refs/remotes/{remote}/{name}");
        if optional_git(cwd, &["rev-parse", "--verify", "--quiet", &reference], None)
            .await
            .is_some_and(|out| !out.is_empty())
        {
            base = Some(name.clone());
        }
    }
    let mut push_options = GitOptions::default().with_timeout(Duration::from_secs(5 * 60));
    for (key, value) in NON_INTERACTIVE {
        push_options = push_options.with_env(key, value);
    }
    if let Err(error) = git(cwd, &["push", "-u", &remote, &branch], Some(&push_options)).await {
        return Err(DhError::msg(format!(
            "git push to {remote} failed: {}",
            first_line(&error)
        )));
    }
    let exec_gh_error = |error: &ExecError| gh_error(&branch, &remote, error.is_not_found(), &first_line(error));
    let mut existing = false;
    match gh(cwd, &["pr", "view", &branch, "--json", "url,state"], None).await {
        Ok(output) => match serde_json::from_str::<Value>(&output.stdout) {
            Ok(view) => existing = view.get("state").and_then(Value::as_str) == Some("OPEN"),
            Err(_) => {
                let message = DhError::json_parse(&output.stdout).message().to_owned();
                let detail = first_line_of(None, &message);
                if !no_pull_request_found(&detail) {
                    return Err(gh_error(&branch, &remote, false, &detail));
                }
            }
        },
        Err(error) => {
            if error.is_not_found() || !no_pull_request_found(&first_line(&error)) {
                return Err(exec_gh_error(&error));
            }
        }
    }
    let mut args: Vec<&str> = if existing {
        vec!["pr", "view", &branch, "--web"]
    } else {
        vec!["pr", "create", "--web", "--head", &branch]
    };
    if !existing && let Some(base) = &base {
        args.extend(["--base", base.as_str()]);
    }
    if let Err(error) = gh(cwd, &args, None).await {
        return Err(exec_gh_error(&error));
    }
    Ok(CreatePrResult {
        branch: branch.clone(),
        remote: remote.clone(),
        base,
        existing,
    })
}

/// TS: workspaceGit.ts changedFiles (porcelain v1 -z: `XY path`, renames/copies followed by their original path).
pub fn changed_files(raw: &str) -> Vec<HandoffChange> {
    let records: Vec<&str> = raw.split('\0').collect();
    let mut changes = Vec::new();
    let mut index = 0;
    while let Some(record) = records.get(index) {
        if jsstr::js_len(record) >= 4 {
            let status = jsstr::js_slice(record, 0, Some(2));
            let trimmed = jsstr::js_trim(&status);
            let rename = matches!(jsstr::js_code_unit_at(&status, 0), Some(0x52 | 0x43)); // R or C
            changes.push(HandoffChange {
                status: if trimmed.is_empty() {
                    status.clone()
                } else {
                    trimmed.to_owned()
                },
                path: jsstr::js_slice(record, 3, None),
            });
            if rename {
                index += 1;
            }
        }
        index += 1;
    }
    changes
}

/// TS: workspaceGit.ts numstatFiles (`a → b` for renames; no counts for binary files).
pub fn numstat_files(raw: &str) -> Vec<HandoffDiffName> {
    let records: Vec<&str> = raw.split('\0').collect();
    let mut files = Vec::new();
    let mut index = 0;
    while let Some(record) = records.get(index) {
        if let Some((added, deleted, file)) = numstat_record(record) {
            let mut name = file.to_owned();
            if name.is_empty() {
                let from = records.get(index + 1).copied().unwrap_or("");
                let to = records.get(index + 2).copied().unwrap_or("");
                name = format!("{from} → {to}");
                index += 2;
            }
            let binary = added == "-";
            files.push(HandoffDiffName {
                path: name,
                additions: (!binary).then(|| digits_number(added)),
                // `Number('-')` is NaN, which JSON writes as null: absent here.
                deletions: (!binary && deleted != "-").then(|| digits_number(deleted)),
            });
        }
        index += 1;
    }
    files
}

/// The fallible part of getHandoffGitContext (its `try` block); errors carry the stderr firstLine prefers.
async fn handoff_context(cwd: &str, base_ref: Option<&str>) -> std::result::Result<HandoffGitContext, HandoffError> {
    if let Some(reference) = base_ref {
        validate_ref(reference).map_err(HandoffError::Dh)?;
    }
    let status_options = GitOptions::default().with_max_buffer(16 * 1024 * 1024);
    let (resolved_base, head, status) = futures::join!(
        base(cwd, base_ref),
        optional_git(cwd, &["rev-parse", "--verify", "--quiet", "HEAD"], None),
        git(
            cwd,
            &[
                "--no-optional-locks",
                "status",
                "--porcelain=v1",
                "-z",
                "--untracked-files=normal"
            ],
            Some(&status_options),
        ),
    );
    let status = status.map_err(HandoffError::Exec)?;
    let changes = changed_files(&status);
    let more_changes = changes.len().saturating_sub(MAX_HANDOFF_CHANGES);
    let mut context = HandoffGitContext {
        base_ref: resolved_base.clone(),
        commits: Vec::new(),
        more_commits: 0,
        changes: changes.into_iter().take(MAX_HANDOFF_CHANGES).collect(),
        more_changes,
        diff: None,
        error: None,
    };
    if let (Some(resolved), Some(_)) = (&resolved_base, head.filter(|head| !head.is_empty())) {
        handoff_commits_exec(cwd, resolved, &mut context).await?;
    }
    Ok(context)
}

/// An error inside getHandoffGitContext's `try`: Git's (with stderr) or Deckhand's own.
enum HandoffError {
    Exec(ExecError),
    Dh(DhError),
}

impl HandoffError {
    fn first_line(&self) -> String {
        match self {
            HandoffError::Exec(error) => first_line(error),
            HandoffError::Dh(error) => first_line_of(None, error.message()),
        }
    }
}

/// [`handoff_commits`] keeping Git's error (and its stderr) for firstLine.
async fn handoff_commits_exec(
    cwd: &str,
    resolved_base: &str,
    context: &mut HandoffGitContext,
) -> std::result::Result<(), HandoffError> {
    let max_count = format!("--max-count={MAX_HANDOFF_COMMITS}");
    let range = format!("{resolved_base}..HEAD");
    let symmetric = format!("{resolved_base}...HEAD");
    let diff_options = GitOptions::default().with_max_buffer(16 * 1024 * 1024);
    let log_args = [
        "log",
        "--no-decorate",
        "--no-color",
        &max_count,
        "--format=%h %s",
        &range,
        "--",
    ];
    let count_args = ["rev-list", "--count", &range];
    let diff_args = [
        "--no-optional-locks",
        "diff",
        "--no-color",
        "--no-ext-diff",
        "--numstat",
        "-z",
        &symmetric,
        "--",
    ];
    let (log, count, diff) = futures::try_join!(
        git(cwd, &log_args, None),
        git(cwd, &count_args, None),
        git(cwd, &diff_args, Some(&diff_options)),
    )
    .map_err(HandoffError::Exec)?;
    context.commits = if log.is_empty() {
        Vec::new()
    } else {
        log.split('\n').map(str::to_owned).collect()
    };
    let count = usize::try_from(count_or_zero(&count)).unwrap_or(0);
    context.more_commits = count.saturating_sub(context.commits.len());
    let names = numstat_files(&diff);
    context.diff = Some(HandoffDiff {
        files: i64::try_from(names.len()).unwrap_or(i64::MAX),
        insertions: names.iter().map(|file| file.additions.unwrap_or(0)).sum(),
        deletions: names.iter().map(|file| file.deletions.unwrap_or(0)).sum(),
        names,
    });
    Ok(())
}

/// TS: workspaceGit.ts getHandoffGitContext (`None` outside a Git repository, unless `cwd` is gone, which is an
/// `error`; other failures are returned as `error` so the export still succeeds).
///
/// Git context for a handoff: commits since base, uncommitted file names with status, and a committed diff stat
/// (`base...HEAD`): names and numbers only, never diff content.
pub async fn get_handoff_git_context(cwd: &str, base_ref: Option<&str>) -> Option<HandoffGitContext> {
    let inside = optional_git(cwd, &["rev-parse", "--is-inside-work-tree"], None).await;
    if inside.as_deref() != Some("true") {
        return match tokio::fs::metadata(cwd).await {
            Ok(_) => None,
            Err(error) => Some(HandoffGitContext {
                error: Some(DhError::io(&error, "stat", cwd).message().to_owned()),
                ..HandoffGitContext::default()
            }),
        };
    }
    // An empty ref is no ref (`if (baseRef)`), but the error record echoes the argument as given.
    let requested = base_ref.filter(|reference| !reference.is_empty());
    match handoff_context(cwd, requested).await {
        Ok(context) => Some(context),
        Err(error) => Some(HandoffGitContext {
            base_ref: base_ref.map(str::to_owned),
            error: Some(error.first_line()),
            ..HandoffGitContext::default()
        }),
    }
}

/// TS: workspaceGit.ts numstatPaths (every path a numstat -z output names, both sides of a rename).
pub fn numstat_paths(raw: &str) -> Vec<String> {
    let records: Vec<&str> = raw.split('\0').collect();
    let mut paths: Vec<String> = Vec::new();
    let mut index = 0;
    while let Some(record) = records.get(index) {
        if let Some((_, _, file)) = numstat_record(record) {
            if file.is_empty() {
                paths.push(records.get(index + 1).copied().unwrap_or("").to_owned());
                paths.push(records.get(index + 2).copied().unwrap_or("").to_owned());
                index += 2;
            } else {
                paths.push(file.to_owned());
            }
        }
        index += 1;
    }
    paths.retain(|file| !file.is_empty());
    paths
}

/// TS: workspaceGit.ts statusPaths (every entry's path and original path).
pub fn status_paths(status: &WorkspaceStatus) -> Vec<String> {
    let mut paths = Vec::new();
    for entry in &status.entries {
        paths.push(entry.path.clone());
        if let Some(orig) = entry.orig_path.as_ref().filter(|orig| !orig.is_empty()) {
            paths.push(orig.clone());
        }
    }
    paths
}

/// Paths the target has uncommitted changes to, deduplicated in order, at most MAX_OVERLAP.
fn touched(paths: &[String], dirty: &HashSet<String>) -> Vec<String> {
    let mut seen: HashSet<&str> = HashSet::new();
    paths
        .iter()
        .filter(|file| dirty.contains(file.as_str()) && seen.insert(file.as_str()))
        .take(MAX_OVERLAP)
        .cloned()
        .collect()
}

/// TS: workspaceGit.ts getMergePreview (read-only, bounded: commits in `target..source`, the committed diff stat,
/// the source's uncommitted files, an operation in progress in the target, overlapping uncommitted files).
pub async fn get_merge_preview(source_cwd: &str, target_cwd: &str) -> Result<MergePreview> {
    let (source_root, target_context) = futures::try_join!(
        async { find_repo_root(Some(source_cwd)).await.map_err(DhError::from) },
        resolve_repo_context(target_cwd),
    )?;
    let target_root = target_context.root.clone();
    if path::resolve(&source_root) == path::resolve(&target_root) {
        return Err(DhError::msg("cannot merge a worktree into itself"));
    }
    let (source_status, target_status, target_branch, default_branch, in_progress) = futures::try_join!(
        read_workspace_status(&source_root, false),
        read_workspace_status(&target_root, false),
        async { current_branch(&target_root).await.map_err(DhError::from) },
        async { Ok::<_, DhError>(resolve_default_branch(&target_root, None, BranchWhere::Local).await) },
        async { Ok::<_, DhError>(operation_in_progress(&target_root).await.ok().flatten()) },
    )?;
    let source_sha = source_status.oid.clone().filter(|oid| !oid.is_empty());
    let source_ref = source_status
        .branch
        .clone()
        .or_else(|| source_status.oid.clone())
        .unwrap_or_else(|| "HEAD".to_owned());
    let target_is_main = target_context
        .main_root
        .as_ref()
        .is_some_and(|main| !main.is_empty() && path::resolve(main) == path::resolve(&target_root));
    let dirty: HashSet<String> = status_paths(&target_status).into_iter().collect();
    let mut preview = MergePreview {
        source_ref,
        source_sha: source_status.oid.clone(),
        target_root: target_root.clone(),
        target_branch: Some(target_branch).filter(|branch| !branch.is_empty()),
        target_is_main,
        default_branch,
        commit_count: 0,
        commits: Vec::new(),
        diff: DiffStat {
            files: 0,
            insertions: 0,
            deletions: 0,
        },
        uncommitted: source_status
            .dirty_files
            .saturating_add(i64::try_from(source_status.untracked.len()).unwrap_or(i64::MAX)),
        overlap: MergeOverlap {
            committed: Vec::new(),
            uncommitted: touched(&status_paths(&source_status), &dirty),
        },
        in_progress,
    };
    let Some(source_sha) = source_sha else {
        return Ok(preview);
    };
    if target_status.oid.as_ref().is_none_or(String::is_empty) {
        return Ok(preview);
    }
    let range = format!("HEAD..{source_sha}");
    let symmetric = format!("HEAD...{source_sha}");
    let max_count = format!("--max-count={MERGE_PREVIEW_COMMITS}");
    let diff_options = GitOptions::default().with_max_buffer(16 * 1024 * 1024);
    let count_args = ["rev-list", "--count", &range];
    let log_args = [
        "log",
        "--no-decorate",
        "--no-color",
        &max_count,
        "--format=%s",
        &range,
        "--",
    ];
    let diff_args = [
        "--no-optional-locks",
        "diff",
        "--no-color",
        "--no-ext-diff",
        "--numstat",
        "-z",
        "-M",
        &symmetric,
        "--",
    ];
    let (count, log, diff) = futures::try_join!(
        git(&target_root, &count_args, None),
        git(&target_root, &log_args, None),
        git(&target_root, &diff_args, Some(&diff_options)),
    )?;
    preview.commit_count = count_or_zero(&count);
    preview.commits = if log.is_empty() {
        Vec::new()
    } else {
        log.split('\n').map(str::to_owned).collect()
    };
    let files = numstat_files(&diff);
    preview.diff = DiffStat {
        files: i64::try_from(files.len()).unwrap_or(i64::MAX),
        insertions: files.iter().map(|file| file.additions.unwrap_or(0)).sum(),
        deletions: files.iter().map(|file| file.deletions.unwrap_or(0)).sum(),
    };
    preview.overlap.committed = touched(&numstat_paths(&diff), &dirty);
    Ok(preview)
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
///
/// Merged outside Deckhand: the tip is reachable from the default branch (local or `origin/<default>`, never
/// fetched) **and** it has a commit of its own beyond its starting point. A fresh branch (no commits: trivially an
/// ancestor of main) never counts, nor one only fast-forwarded or reset to a newer main (its reflog shows no commit
/// made on it). One Git call; the second and third only once the tip looks merged.
pub async fn merged_into_default(check: &AncestryCheck) -> bool {
    let AncestryCheck {
        cwd,
        tip,
        branch,
        start,
        defaults,
    } = check;
    if defaults.is_empty() || !is_sha(tip) || !is_sha(start) || tip == start {
        return false;
    }
    let options = GitOptions::default().with_timeout(Duration::from_millis(5000));
    let mut args: Vec<&str> = vec!["rev-list", "--max-count=1", tip, "--not", start];
    args.extend(defaults.iter().map(String::as_str));
    args.push("--");
    match optional_git(cwd, &args, Some(&options)).await {
        Some(unmerged) if jsstr::js_trim(&unmerged).is_empty() => {}
        _ => return false,
    }
    // A commit beyond the start: tip descends from it (and differs). A tip reset behind its start does not.
    if optional_git(cwd, &["merge-base", "--is-ancestor", start, tip], Some(&options))
        .await
        .is_none()
    {
        return false;
    }
    if branch.is_empty() {
        return true;
    }
    branch_has_own_commits(cwd, branch).await != Some(false)
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
/// Bounded: two Git calls.
pub async fn worktree_activity(cwd: &str, defaults: &[String]) -> WorktreeActivity {
    let options = GitOptions::default().with_timeout(Duration::from_millis(5000));
    let time = optional_git(cwd, &["log", "-1", "--format=%cI", "HEAD", "--"], Some(&options));
    let ahead = async {
        if defaults.is_empty() {
            return None;
        }
        let mut args: Vec<&str> = vec!["rev-list", "--count", "HEAD", "--not"];
        args.extend(defaults.iter().map(String::as_str));
        args.push("--");
        optional_git(cwd, &args, Some(&options)).await
    };
    let (time, ahead) = futures::join!(time, ahead);
    WorktreeActivity {
        last_commit_at: time.filter(|time| !time.is_empty()),
        ahead_of_default: ahead
            .filter(|ahead| !ahead.is_empty() && ahead.bytes().all(|byte| byte.is_ascii_digit()))
            .map(|ahead| digits_number(&ahead)),
    }
}

/// tests/helpers.ts `repo()` / `git()` for this module's and changes_git's tests.
#[cfg(test)]
pub(crate) mod test_repo {
    use std::path::Path;
    use std::process::Command;

    /// tests/helpers.ts `git(cwd, ...args)`: trimmed stdout (the user's Git config ignored).
    pub fn run_git(dir: &str, args: &[&str]) -> std::result::Result<String, String> {
        let output = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .map_err(|error| error.to_string())?;
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        if output.status.success() {
            Ok(stdout)
        } else {
            Err(format!("{stdout}{}", String::from_utf8_lossy(&output.stderr)))
        }
    }

    /// Like [`run_git`], panicking on failure.
    pub fn tgit(dir: &str, args: &[&str]) -> String {
        match run_git(dir, args) {
            Ok(stdout) => stdout,
            Err(error) => panic!("git {args:?} failed: {error}"),
        }
    }

    pub fn real(dir: &Path) -> String {
        std::fs::canonicalize(dir).unwrap().to_string_lossy().into_owned()
    }

    pub fn write(dir: &str, file: &str, text: impl AsRef<[u8]>) {
        std::fs::write(Path::new(dir).join(file), text).unwrap();
    }

    /// A temp directory (`fs.mkdtemp`) and its canonical path.
    pub fn temp(prefix: &str) -> (tempfile::TempDir, String) {
        let dir = tempfile::Builder::new().prefix(prefix).tempdir().unwrap();
        let path = real(dir.path());
        (dir, path)
    }

    /// tests/helpers.ts `repo()`: `main` with one commit of `file.txt`.
    pub fn repo() -> (tempfile::TempDir, String) {
        let (dir, path) = temp("deckhand-test-");
        tgit(&path, &["init", "-b", "main"]);
        tgit(&path, &["config", "user.name", "Deckhand Test"]);
        tgit(&path, &["config", "user.email", "deckhand@example.invalid"]);
        tgit(&path, &["config", "commit.gpgsign", "false"]);
        let no_hooks = Path::new(&path).join(".git").join("no-hooks");
        tgit(&path, &["config", "core.hooksPath", &no_hooks.to_string_lossy()]);
        write(&path, "file.txt", "first\n");
        tgit(&path, &["add", "."]);
        tgit(&path, &["commit", "-m", "initial"]);
        (dir, path)
    }
}

#[cfg(test)]
mod tests {
    use super::test_repo::{repo, temp, tgit, write};
    use super::*;
    use deckhand_testkit::tags::{self, JsValue};
    use deckhand_testkit::{Case, replay};
    use std::path::Path;

    // Lone UTF-16 surrogates cannot live in a Rust String (and Git output is UTF-8, so the port never sees them). The
    // replay maps each one to a private-use character (one UTF-16 unit as well, so slicing is unchanged) and back.
    const LONE_BASE: u32 = 0xE000;

    fn from_units(units: &[u16]) -> String {
        char::decode_utf16(units.iter().copied())
            .map(|unit| match unit {
                Ok(ch) => ch,
                Err(error) => {
                    char::from_u32(LONE_BASE + u32::from(error.unpaired_surrogate()) - 0xD800).unwrap_or('\u{FFFD}')
                }
            })
            .collect()
    }

    fn restore_lone(value: Value) -> Value {
        match value {
            Value::String(text) => {
                let lone = |ch: char| (LONE_BASE..LONE_BASE + 0x800).contains(&u32::from(ch));
                if !text.chars().any(lone) {
                    return Value::String(text);
                }
                let mut units: Vec<u16> = Vec::new();
                for ch in text.chars() {
                    if lone(ch) {
                        units.push(u16::try_from(u32::from(ch) - LONE_BASE + 0xD800).unwrap());
                    } else {
                        let mut buf = [0u16; 2];
                        units.extend_from_slice(ch.encode_utf16(&mut buf));
                    }
                }
                serde_json::json!({"$utf16": units})
            }
            Value::Array(items) => Value::Array(items.into_iter().map(restore_lone).collect()),
            Value::Object(map) => {
                Value::Object(map.into_iter().map(|(key, value)| (key, restore_lone(value))).collect())
            }
            other => other,
        }
    }

    #[test]
    fn parse_status_parity() {
        replay(
            "workspaceGit/parseStatus",
            |case: &Case| -> std::result::Result<Value, String> {
                let raw = match tags::decode(case.arg(0))? {
                    JsValue::String(text) => text,
                    JsValue::Utf16(units) => from_units(&units),
                    other => return Err(format!("unexpected argument {other:?}")),
                };
                let value = serde_json::to_value(parse_status(&raw)).map_err(|error| error.to_string())?;
                Ok(restore_lone(value))
            },
        );
    }

    #[test]
    fn numstat_helpers() {
        let raw = "1\t2\ta.txt\0-\t-\tbin\0\x003\t0\t\0old\0new\0";
        assert_eq!(
            numstat("3\t1\ta\0-\t-\tb\0\x005\t2\t\0x\0y\0"),
            NumstatTotals {
                additions: 8,
                deletions: 3
            }
        );
        assert_eq!(numstat_paths(raw), vec!["a.txt", "bin", "old", "new"]);
        let files = numstat_files(raw);
        assert_eq!(files.len(), 3);
        assert_eq!(files[1].additions, None);
        assert_eq!(files[2].path, "old → new");
        assert_eq!(files[2].additions, Some(3));
        let changes = changed_files("R  new\0old\0 M a b\0?? c\0x\0");
        let shown: Vec<(&str, &str)> = changes.iter().map(|c| (c.status.as_str(), c.path.as_str())).collect();
        assert_eq!(shown, vec![("R", "new"), ("M", "a b"), ("??", "c")]);
        assert!(validate_ref("main").is_ok());
        for bad in ["", "-x", "a\nb", "a\0b", "a\rb"] {
            assert_eq!(validate_ref(bad).unwrap_err().to_string(), "Invalid Git comparison ref");
        }
        assert!(disposable_ignored("a/node_modules/b"));
        assert!(!disposable_ignored("node_modules_x/b"));
    }

    #[test]
    fn pull_request_json() {
        let sha = "a".repeat(40);
        let pr = pull_request_from_json(&format!(
            r#"{{"number":12,"url":"https://x/pr/12","state":"OPEN","statusCheckRollup":[{{"conclusion":"SUCCESS"}},{{"state":"SKIPPED"}}],"headRefOid":"{sha}","baseRefName":"main"}}"#
        ))
        .unwrap();
        assert_eq!(pr.checks, PrChecks::Passing);
        assert_eq!(pr.head_sha.as_deref(), Some(sha.as_str()));
        assert_eq!(pr.base_branch.as_deref(), Some("main"));
        let pending = pull_request_from_json(
            r#"{"number":1,"url":"https://x","state":3,"statusCheckRollup":[{"status":"QUEUED"}],"baseRefName":"-x"}"#,
        )
        .unwrap();
        assert_eq!(
            (pending.checks, pending.state.as_str(), pending.base_branch),
            (PrChecks::Pending, "3", None)
        );
        let failing = pull_request_from_json(
            r#"{"number":1,"url":"https://x","statusCheckRollup":[{"conclusion":"FAILURE"},{"state":"PENDING"}]}"#,
        )
        .unwrap();
        assert_eq!(
            (failing.checks, failing.state.as_str()),
            (PrChecks::Failing, "undefined")
        );
        let unknown = pull_request_from_json(r#"{"number":1,"url":"https://x","statusCheckRollup":[]}"#).unwrap();
        assert_eq!(unknown.checks, PrChecks::Unknown);
        for bad in [
            r#"{"number":1.5,"url":"https://x"}"#,
            r#"{"number":1,"url":"http://x"}"#,
            "5",
        ] {
            assert_eq!(
                pull_request_from_json(bad).unwrap_err().to_string(),
                "Invalid PR response"
            );
        }
        assert_eq!(
            pull_request_from_json("null").unwrap_err().to_string(),
            "Cannot read properties of null (reading 'number')"
        );
        assert_eq!(
            pull_request_from_json("").unwrap_err().to_string(),
            "Unexpected end of JSON input"
        );
    }

    #[test]
    fn porcelain_v2_parsing() {
        let raw = [
            "# branch.oid (initial)",
            "# branch.head (detached)",
            "# branch.upstream origin/x",
            "# branch.ab +2 -1",
            "2 R. N... 100644 100644 100644 aaa bbb R100 new name\nwith newline",
            "old 1 M. x",
            "1 .M N... 100644 100644 100644 aaa aaa spaced 2 name",
            "u UU N... 100644 100644 100644 100644 a b c conflict",
            "? untracked\nname",
            "! node_modules/",
            "",
        ]
        .join("\0");
        let entry = |kind, xy: &str, path: &str, orig: Option<&str>| StatusEntry {
            kind,
            xy: xy.to_owned(),
            path: path.to_owned(),
            orig_path: orig.map(str::to_owned),
        };
        assert_eq!(
            parse_status(&raw),
            WorkspaceStatus {
                oid: None,
                branch: None,
                upstream: Some("origin/x".to_owned()),
                ahead: Some(2),
                behind: Some(1),
                dirty_files: 3,
                untracked: vec!["untracked\nname".to_owned()],
                ignored: vec!["node_modules/".to_owned()],
                entries: vec![
                    entry(StatusKind::Renamed, "R.", "new name\nwith newline", Some("old 1 M. x")),
                    entry(StatusKind::Ordinary, ".M", "spaced 2 name", None),
                    entry(StatusKind::Unmerged, "UU", "conflict", None),
                    entry(StatusKind::Untracked, "??", "untracked\nname", None),
                ],
            }
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cleanup_blocks_dirty_untracked_and_valuable_ignored() {
        let (_dir, cwd) = repo();
        write(&cwd, ".gitignore", ".env\nnode_modules/\ntarget/\n");
        tgit(&cwd, &["add", ".gitignore"]);
        tgit(&cwd, &["commit", "-m", "ignore"]);
        write(&cwd, ".env", "secret");
        std::fs::create_dir(Path::new(&cwd).join("node_modules")).unwrap();
        write(&cwd, "node_modules/cache", "cache");
        std::fs::create_dir_all(Path::new(&cwd).join("target/deep")).unwrap();
        for i in 0..50 {
            write(&cwd, &format!("target/deep/{i}"), "");
        }
        write(&cwd, "file.txt", "modified\n");
        write(&cwd, "untracked.txt", "valuable");
        let inspection = inspect_workspace_cleanup(&cwd, Some("main"), &CleanupOptions::default())
            .await
            .unwrap();
        assert_eq!(inspection.dirty_files, 1);
        assert_eq!(inspection.untracked_files, 1);
        assert_eq!(inspection.ignored_files, 2);
        assert!(!inspection.safe);
        assert!(
            inspection
                .reasons
                .iter()
                .any(|reason| reason.contains(".env") && reason.contains("target/"))
        );
        // The files themselves, for the confirmation: changed first, then untracked, then valuable ignored (no
        // dependency cache).
        let file = |path: &str, state| CleanupFile {
            path: path.to_owned(),
            state,
        };
        assert_eq!(
            inspection.files,
            Some(vec![
                file("file.txt", CleanupFileState::Changed),
                file("untracked.txt", CleanupFileState::Untracked),
                file(".env", CleanupFileState::Ignored),
                file("target/", CleanupFileState::Ignored),
            ])
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn summary_separates_changes_from_commits() {
        let (_dir, cwd) = repo();
        tgit(&cwd, &["checkout", "-b", "feature"]);
        tgit(&cwd, &["branch", "--set-upstream-to=main"]);
        write(&cwd, "file.txt", "first\nsecond\n");
        tgit(&cwd, &["add", "."]);
        tgit(&cwd, &["commit", "-m", "feature"]);
        write(&cwd, "file.txt", "first\nsecond\nthird\n");
        tgit(&cwd, &["add", "file.txt"]);
        write(&cwd, "file.txt", "first\nsecond\nthird\nfourth\n");
        let summary = get_workspace_summary(&cwd, Some("main"), false, None).await.unwrap();
        assert_eq!(summary.branch, "feature");
        assert_eq!(summary.changed_files, 1);
        assert_eq!(summary.additions, 2);
        assert_eq!(summary.commits_ahead_of_base, Some(1));
        assert_eq!(summary.ahead, Some(1));
        assert_eq!(summary.behind, Some(0));
        let inspection = inspect_workspace_cleanup(&cwd, Some("main"), &CleanupOptions::default())
            .await
            .unwrap();
        assert_eq!(inspection.unpublished_commits, Some(1));
        assert_eq!(inspection.unmerged_commits, Some(1));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn commits_only_block_when_they_would_be_lost() {
        let (_dir, cwd) = repo();
        tgit(&cwd, &["checkout", "-b", "feature"]);
        let keep = CleanupOptions::default();
        let delete = CleanupOptions {
            delete_branch: true,
            integrated: None,
        };
        // Never pushed, no new commits.
        assert!(inspect_workspace_cleanup(&cwd, None, &keep).await.unwrap().safe);
        // main still has them.
        assert!(inspect_workspace_cleanup(&cwd, None, &delete).await.unwrap().safe);
        write(&cwd, "file.txt", "feature\n");
        tgit(&cwd, &["commit", "-am", "feature"]);
        // The branch preserves the commit.
        assert!(inspect_workspace_cleanup(&cwd, Some("main"), &keep).await.unwrap().safe);
        let deleting = inspect_workspace_cleanup(&cwd, Some("main"), &delete).await.unwrap();
        assert!(!deleting.safe);
        assert!(
            deleting
                .reasons
                .join(",")
                .contains("1 commit(s) exist only on branch feature")
        );
        tgit(&cwd, &["checkout", "--detach"]);
        write(&cwd, "file.txt", "detached\n");
        tgit(&cwd, &["commit", "-am", "detached"]);
        let detached = inspect_workspace_cleanup(&cwd, None, &keep).await.unwrap();
        assert!(detached.reasons.join(",").contains("detached HEAD"));
        let summary = get_workspace_summary(&cwd, Some("--all"), false, None).await;
        assert!(summary.unwrap_err().to_string().contains("Invalid Git"));
        let inspection = inspect_workspace_cleanup(&cwd, Some("--all"), &keep).await;
        assert!(inspection.unwrap_err().to_string().contains("Invalid Git"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn symlinked_node_modules_and_unborn_repositories() {
        let (_dir, cwd) = repo();
        let (_shared_dir, shared) = repo();
        std::os::unix::fs::symlink(&shared, Path::new(&cwd).join("node_modules")).unwrap();
        assert!(
            inspect_workspace_cleanup(&cwd, None, &CleanupOptions::default())
                .await
                .unwrap()
                .safe
        );
        tgit(&cwd, &["checkout", "--orphan", "unborn"]);
        tgit(&cwd, &["rm", "-f", "file.txt"]);
        let summary = get_workspace_summary(&cwd, Some("missing"), false, None).await.unwrap();
        assert_eq!(summary.head, "");
        assert_eq!(summary.base_ref, None);
        assert_eq!(summary.branch, "unborn");
        let gone = format!("{cwd}/gone");
        let missing = inspect_workspace_cleanup(&gone, None, &CleanupOptions::default())
            .await
            .unwrap();
        assert!(!missing.safe);
        assert!(missing.reasons[0].contains("Git status unavailable"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn summary_pr_lookup_and_handoff_context() {
        let (_dir, cwd) = repo();
        tgit(&cwd, &["checkout", "-b", "feature"]);
        write(&cwd, "file.txt", "first\nchanged\n");
        tgit(&cwd, &["commit", "-am", "change it"]);
        write(&cwd, "new.txt", "x\n");
        let missing_gh: PullRequestLookup =
            Rc::new(|_| Box::pin(async { Err(DhError::with_code("ENOENT", "spawn gh ENOENT")) }));
        let summary = get_workspace_summary(&cwd, None, true, Some(missing_gh)).await.unwrap();
        assert_eq!(summary.pr_error.as_deref(), Some("Install gh for PR status"));
        assert_eq!(summary.commits_ahead_of_base, None);
        let failing: PullRequestLookup = Rc::new(|_| Box::pin(async { Err(DhError::msg("no")) }));
        let summary = get_workspace_summary(&cwd, None, true, Some(failing)).await.unwrap();
        assert_eq!(
            summary.pr_error.as_deref(),
            Some("No PR status available; check gh authentication and branch")
        );
        let context = get_handoff_git_context(&cwd, Some("main")).await.unwrap();
        assert_eq!(context.base_ref.as_deref(), Some("main"));
        assert_eq!(context.commits.len(), 1);
        assert!(context.commits[0].ends_with(" change it"));
        assert_eq!(
            context.changes,
            vec![HandoffChange {
                status: "??".to_owned(),
                path: "new.txt".to_owned()
            }]
        );
        let diff = context.diff.unwrap();
        assert_eq!((diff.files, diff.insertions, diff.deletions), (1, 1, 0));
        let invalid = get_handoff_git_context(&cwd, Some("-x")).await.unwrap();
        assert_eq!(invalid.error.as_deref(), Some("Invalid Git comparison ref"));
        assert_eq!(invalid.base_ref.as_deref(), Some("-x"));
        let gone = format!("{cwd}/gone");
        let missing = get_handoff_git_context(&gone, None).await.unwrap();
        assert_eq!(
            missing.error,
            Some(format!("ENOENT: no such file or directory, stat '{gone}'"))
        );
        let (_plain_dir, plain) = temp("deckhand-plain-");
        assert_eq!(get_handoff_git_context(&plain, None).await, None);
        let activity = worktree_activity(&cwd, &["refs/heads/main".to_owned()]).await;
        assert_eq!(activity.ahead_of_default, Some(1));
        assert!(activity.last_commit_at.is_some());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn merge_preview_and_merged_detection() {
        let (_dir, root) = repo();
        let (_parent, parent) = temp("deckhand-wt-");
        let worktree = format!("{parent}/wt");
        tgit(&root, &["worktree", "add", "-b", "feature", &worktree]);
        let start = tgit(&root, &["rev-parse", "HEAD"]);
        write(&worktree, "file.txt", "first\nfeature\n");
        tgit(&worktree, &["commit", "-am", "feature work"]);
        write(&worktree, "extra.txt", "x\n");
        write(&root, "file.txt", "dirty\n");
        let preview = get_merge_preview(&worktree, &root).await.unwrap();
        assert_eq!(preview.source_ref, "feature");
        assert!(preview.target_is_main);
        assert_eq!(preview.target_branch.as_deref(), Some("main"));
        assert_eq!(preview.default_branch.as_deref(), Some("main"));
        assert_eq!(preview.commit_count, 1);
        assert_eq!(preview.commits, vec!["feature work"]);
        assert_eq!((preview.diff.files, preview.diff.insertions), (1, 1));
        assert_eq!(preview.uncommitted, 1);
        assert_eq!(preview.overlap.committed, vec!["file.txt"]);
        assert!(preview.overlap.uncommitted.is_empty());
        assert_eq!(
            get_merge_preview(&root, &root).await.unwrap_err().to_string(),
            "cannot merge a worktree into itself"
        );
        let tip = tgit(&worktree, &["rev-parse", "HEAD"]);
        let check = AncestryCheck {
            cwd: root.clone(),
            tip: tip.clone(),
            branch: "feature".to_owned(),
            start: start.clone(),
            defaults: vec!["refs/heads/main".to_owned()],
        };
        assert!(!merged_into_default(&check).await);
        tgit(&root, &["checkout", "--", "file.txt"]);
        tgit(&root, &["merge", "--ff-only", "feature"]);
        assert!(merged_into_default(&check).await);
        let fresh = AncestryCheck {
            tip: start.clone(),
            ..check.clone()
        };
        assert!(!merged_into_default(&fresh).await);
    }

    struct MergeFixture {
        _root_dir: tempfile::TempDir,
        parent: tempfile::TempDir,
        root: String,
    }

    impl MergeFixture {
        fn new() -> Self {
            let (root_dir, root) = repo();
            let (parent, _) = temp("deckhand-merge-wt-");
            write(&root, "other.txt", "other\n");
            tgit(&root, &["add", "."]);
            tgit(&root, &["commit", "-m", "other"]);
            MergeFixture {
                _root_dir: root_dir,
                parent,
                root,
            }
        }

        fn worktree(&self, name: &str) -> String {
            let dir = self.parent.path().join(name);
            tgit(&self.root, &["worktree", "add", "-b", name, &dir.to_string_lossy()]);
            super::test_repo::real(&dir)
        }
    }

    fn commit_file(dir: &str, file: &str, text: &str, message: &str) {
        write(dir, file, text);
        tgit(dir, &["add", "-A"]);
        tgit(dir, &["commit", "-m", message]);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn squash_merge_cleanup_counts_integrated_commits() {
        use crate::git::{MergeOptions, merge_worktree_into_current};
        use deckhand_protocol::data::WorktreeMergeMode;
        let fx = MergeFixture::new();
        let wt = fx.worktree("squashed");
        commit_file(&wt, "a.txt", "a\n", "one");
        commit_file(&wt, "b.txt", "b\n", "two");
        let result = merge_worktree_into_current(&wt, &fx.root, WorktreeMergeMode::Squash, &MergeOptions::default())
            .await
            .unwrap();
        tgit(&fx.root, &["commit", "-m", "squashed"]);
        let source_sha = result.result.source_sha.clone();
        let inspect = |integrated: Option<String>| {
            let wt = wt.clone();
            async move {
                let options = CleanupOptions {
                    delete_branch: true,
                    integrated,
                };
                inspect_workspace_cleanup(&wt, Some("main"), &options).await.unwrap()
            }
        };
        // Git does not consider the branch merged: without the recorded commit, deleting the branch would "lose" both.
        let plain = inspect(None).await;
        assert!(
            plain
                .reasons
                .join(",")
                .contains("2 commit(s) exist only on branch squashed")
        );
        let integrated = inspect(source_sha.clone()).await;
        assert!(integrated.safe, "{}", integrated.reasons.join(","));
        commit_file(&wt, "c.txt", "c\n", "after the merge");
        let later = inspect(source_sha).await;
        assert!(
            later
                .reasons
                .join(",")
                .contains("1 commit(s) exist only on branch squashed")
        );
        // A recorded commit that no longer exists (or is not a SHA) is ignored, never trusted.
        for bogus in ["f".repeat(40), "--all".to_owned()] {
            let ignored = inspect(Some(bogus)).await;
            assert!(
                ignored.reasons.join(",").contains("3 commit(s)"),
                "{:?}",
                ignored.reasons
            );
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn merge_detection_counts_only_own_merged_commits() {
        use crate::git::branch_creation_commit;
        let fx = MergeFixture::new();
        let root = fx.root.as_str();
        let defaults = vec!["refs/heads/main".to_owned()];
        let check = |wt: &str, branch: &str, start: &str, refs: &[String]| {
            let check = AncestryCheck {
                cwd: wt.to_owned(),
                tip: tgit(wt, &["rev-parse", "HEAD"]),
                branch: branch.to_owned(),
                start: start.to_owned(),
                defaults: refs.to_vec(),
            };
            async move { merged_into_default(&check).await }
        };
        // Fresh: no commits beyond its start (trivially an ancestor of main).
        let fresh = fx.worktree("fresh");
        let fresh_start = branch_creation_commit(&fresh, "fresh").await.unwrap();
        assert_eq!(fresh_start, tgit(&fresh, &["rev-parse", "HEAD"]));
        assert!(!check(&fresh, "fresh", &fresh_start, &defaults).await);
        // Fresh, then brought up to date with a newer main (fast-forward): still nothing of its own.
        commit_file(root, "main.txt", "m\n", "main moves on");
        tgit(&fresh, &["merge", "--ff-only", "main"]);
        assert!(!check(&fresh, "fresh", &fresh_start, &defaults).await);

        // A branch with a commit: not merged, then merged into main (here with a merge commit).
        let feat = fx.worktree("feat");
        let start = branch_creation_commit(&feat, "feat").await.unwrap();
        commit_file(&feat, "f.txt", "f\n", "feature");
        assert!(!check(&feat, "feat", &start, &defaults).await);
        tgit(root, &["merge", "--no-ff", "-m", "merge feat", "feat"]);
        assert!(check(&feat, "feat", &start, &defaults).await);
        // Commits after that merge are not merged.
        commit_file(&feat, "g.txt", "g\n", "more");
        assert!(!check(&feat, "feat", &start, &defaults).await);

        // Merged only on the remote (origin/main, never fetched by Deckhand): counts when that ref is given.
        let remote = fx.worktree("remote");
        let remote_start = branch_creation_commit(&remote, "remote").await.unwrap();
        commit_file(&remote, "r.txt", "r\n", "remote work");
        let remote_tip = tgit(&remote, &["rev-parse", "HEAD"]);
        tgit(root, &["update-ref", "refs/remotes/origin/main", &remote_tip]);
        let with_remote = vec!["refs/heads/main".to_owned(), "refs/remotes/origin/main".to_owned()];
        assert!(!check(&remote, "remote", &remote_start, &defaults).await);
        assert!(check(&remote, "remote", &remote_start, &with_remote).await);
        // No starting point or no default refs: never.
        assert!(!check(&remote, "remote", "not-a-sha", &with_remote).await);
        assert!(!check(&remote, "remote", &remote_start, &[]).await);
        // A branch reset behind where it started is not "beyond" it.
        let reset = fx.worktree("reset");
        let reset_start = branch_creation_commit(&reset, "reset").await.unwrap();
        tgit(&reset, &["reset", "--hard", "HEAD~1"]);
        assert!(!check(&reset, "reset", &reset_start, &defaults).await);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn pr_lookup_is_injectable() {
        use std::cell::RefCell;
        let fx = MergeFixture::new();
        let wt = fx.worktree("pr-branch");
        commit_file(&wt, "p.txt", "p\n", "pr work");
        let head_sha = tgit(&wt, &["rev-parse", "HEAD"]);
        let asked = Rc::new(RefCell::new(String::new()));
        let expected = PullRequestInfo {
            number: 7,
            url: "https://github.com/acme/app/pull/7".to_owned(),
            state: "MERGED".to_owned(),
            checks: PrChecks::Passing,
            head_sha: Some(head_sha),
            base_branch: Some("main".to_owned()),
        };
        let lookup: PullRequestLookup = {
            let asked = asked.clone();
            let expected = expected.clone();
            Rc::new(move |cwd| {
                *asked.borrow_mut() = cwd;
                let expected = expected.clone();
                Box::pin(async move { Ok(expected) })
            })
        };
        let summary = get_workspace_summary(&wt, Some("main"), true, Some(lookup))
            .await
            .unwrap();
        assert_eq!(*asked.borrow(), wt);
        assert_eq!(summary.pr, Some(expected));
        let enoent: PullRequestLookup =
            Rc::new(|_| Box::pin(async { Err(DhError::with_code("ENOENT", "spawn gh ENOENT")) }));
        let missing = get_workspace_summary(&wt, Some("main"), true, Some(enoent))
            .await
            .unwrap();
        assert_eq!(missing.pr, None);
        assert_eq!(missing.pr_error.as_deref(), Some("Install gh for PR status"));
        // Without includePr the lookup never runs.
        let never: PullRequestLookup = Rc::new(|_| Box::pin(async { panic!("must not run") }));
        get_workspace_summary(&wt, Some("main"), false, Some(never))
            .await
            .unwrap();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn handoffs_list_subjects_and_names_never_diff_content() {
        use deckhand_core::session_features::handoff_markdown;
        use deckhand_protocol::data::SessionRecord;
        let (_dir, cwd) = repo();
        tgit(&cwd, &["checkout", "-b", "feature"]);
        write(&cwd, "file.txt", "COMMITTED_DIFF_LINE\n");
        tgit(&cwd, &["commit", "-am", "Add the feature"]);
        write(&cwd, "notes.md", "UNCOMMITTED_CONTENT\n");
        let session = |program: &str, notes: Option<&str>, title: &str| -> SessionRecord {
            let mut value = serde_json::json!({
                "id": "s", "title": title, "program": program, "command": "", "cwd": cwd, "repoRoot": "",
                "status": "running", "createdAt": "", "updatedAt": "",
            });
            if let Some(notes) = notes {
                value["notes"] = serde_json::json!(notes);
            }
            serde_json::from_value(value).unwrap()
        };
        let context = get_handoff_git_context(&cwd, Some("main")).await;
        let markdown = handoff_markdown(&session("claude", Some("n"), "Task"), false, context.as_ref(), None);
        assert!(markdown.contains("## Workspace changes"), "{markdown}");
        assert!(markdown.contains("Base: main"));
        let commit_line = |line: &&str| {
            line.strip_prefix("- ")
                .and_then(|rest| rest.split_once(' '))
                .is_some_and(|(sha, subject)| {
                    !sha.is_empty() && sha.chars().all(|c| c.is_ascii_hexdigit()) && subject == "Add the feature"
                })
        };
        assert!(markdown.lines().any(|line| commit_line(&line)), "{markdown}");
        assert!(markdown.contains("- ?? notes.md"));
        assert!(markdown.contains("1 file(s) changed, 1 insertion(s), 1 deletion(s)"));
        assert!(markdown.contains("- file.txt +1 -1"));
        assert!(!markdown.contains("COMMITTED_DIFF_LINE") && !markdown.contains("UNCOMMITTED_CONTENT"));
        // Outside a repository: omitted.
        let outside = Path::new(&cwd).parent().unwrap().to_string_lossy().into_owned();
        assert_eq!(get_handoff_git_context(&outside, None).await, None);
        let invalid = get_handoff_git_context(&cwd, Some("--all")).await;
        let unavailable = handoff_markdown(&session("pi", None, "T"), false, invalid.as_ref(), None);
        assert!(
            unavailable.contains("Git information unavailable: Invalid Git"),
            "{unavailable}"
        );
    }
}
