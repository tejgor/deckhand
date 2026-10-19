//! Port of git.ts.
//!
//! Git plumbing for sessions and worktrees: bounded `git -C <cwd>` runs ([`git`], [`optional_git`]), repository
//! context, the worktree list, creating a session's worktree (a trusted creation hook, or Deckhand's own `git worktree
//! add` with worktree settings), removing and pruning worktrees, and merging a worktree into the launch checkout
//! without committing.
//!
//! Errors: Git failures are [`ExecError`]s (Node's execFile error: `Command failed: git -C ...\n<stderr>`, with
//! `stdout`/`stderr` kept, which callers fold into their messages); Deckhand's own refusals are [`DhError`]s with the
//! TS message. `ExecError` converts into `DhError` with `?`.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;

use crate::proc::ExecError;
use deckhand_core::error::Result;
use deckhand_protocol::Ordered;
use deckhand_protocol::data::{
    BranchFrom, CommittedChanges, CreationHook, TargetOperation, TemplateVars, WorktreeCreator, WorktreeLinks,
    WorktreeMergeMode, WorktreeMergeResult, WorktreeOrigin, WorktreeSettings,
};

/// TS: git.ts operationProblem (why a merge must not start on top of what the target is doing); pure, shared with
/// the merge screen.
pub use deckhand_core::merge_model::operation_problem;

/// TS: git.ts GitOptions (`timeout` default 8 s, `maxBuffer` default 8 MiB; `env`: variables added to the daemon's
/// environment, TS `{...process.env, ...}`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitOptions {
    pub timeout: Duration,
    pub max_buffer: usize,
    pub env: Vec<(String, String)>,
}

impl Default for GitOptions {
    fn default() -> Self {
        GitOptions {
            timeout: Duration::from_millis(8000),
            max_buffer: 8 * 1024 * 1024,
            env: Vec::new(),
        }
    }
}

impl GitOptions {
    /// TS: git.ts SLOW (mutating runs: 10 minutes, 16 MiB).
    pub fn slow() -> Self {
        GitOptions {
            timeout: Duration::from_secs(10 * 60),
            max_buffer: 16 * 1024 * 1024,
            env: Vec::new(),
        }
    }

    /// The same options with a different timeout.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// The same options with a different byte limit per stream.
    pub fn with_max_buffer(mut self, max_buffer: usize) -> Self {
        self.max_buffer = max_buffer;
        self
    }

    /// The same options with an environment variable added.
    pub fn with_env(mut self, key: &str, value: &str) -> Self {
        self.env.push((key.to_owned(), value.to_owned()));
        self
    }
}

/// TS: git.ts NON_INTERACTIVE_GIT (no credential prompts for fetches).
pub const NON_INTERACTIVE_GIT: [(&str, &str); 2] = [("GIT_TERMINAL_PROMPT", "0"), ("GCM_INTERACTIVE", "never")];

/// TS: git.ts CREATE_WORKTREE_SCRIPT (relative to a checkout).
pub const CREATE_WORKTREE_SCRIPT: &str = ".claude/scripts/create-worktree.sh";

/// TS: git.ts MAX_CONFLICT_PATHS (conflicted paths returned at most; the count is exact).
pub const MAX_CONFLICT_PATHS: usize = 200;

/// TS: git.ts gitOutput result.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GitOutput {
    pub stdout: String,
    pub stderr: String,
}

/// TS: git.ts gitOutput (`git -C <cwd> <args>`, bounded; `options` default [`GitOptions::default`]).
pub async fn git_output(
    cwd: &str,
    args: &[&str],
    options: Option<&GitOptions>,
) -> std::result::Result<GitOutput, ExecError> {
    let _ = (cwd, args, options);
    todo!("I3")
}

/// TS: git.ts git (stdout with trailing whitespace trimmed).
pub async fn git(cwd: &str, args: &[&str], options: Option<&GitOptions>) -> std::result::Result<String, ExecError> {
    let _ = (cwd, args, options);
    todo!("I3")
}

/// TS: git.ts optionalGit (`None` on any failure).
pub async fn optional_git(cwd: &str, args: &[&str], options: Option<&GitOptions>) -> Option<String> {
    let _ = (cwd, args, options);
    todo!("I3")
}

/// TS: git.ts WorktreeInfo (one checkout of `git worktree list`; the first is the main worktree).
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeInfo {
    pub path: String,
    pub branch: String,
    pub head: String,
    pub is_main: bool,
    /// `git worktree lock`ed: Git refuses to remove it without a second --force.
    pub locked: Option<bool>,
    /// Listed, but its directory is gone (`git worktree prune` drops it).
    pub prunable: Option<bool>,
}

/// TS: git.ts CreatedWorktreeInfo (`{...metadata, path, origin, creator, name, links?, baseRef?}`).
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreatedWorktreeInfo {
    pub path: String,
    pub branch: String,
    pub head: String,
    pub is_main: bool,
    pub locked: Option<bool>,
    pub prunable: Option<bool>,
    /// `created` or `existing` (never `selected` here).
    pub origin: WorktreeOrigin,
    /// `script` (the creation hook) or `fallback` (Deckhand's `git worktree add`).
    pub creator: WorktreeCreator,
    pub name: String,
    /// Symlinks applied from worktree settings; only for a newly created worktree.
    pub links: Option<WorktreeLinks>,
    /// The base a new branch was started from (branch name, `origin/<default>` or a commit).
    pub base_ref: Option<String>,
}

/// TS: git.ts RepoContext.
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoContext {
    /// Checkout containing cwd.
    pub root: String,
    pub common_dir: String,
    /// Main checkout; `None` for bare repositories, which have no shared checkout.
    pub main_root: Option<String>,
    /// Per-repository key for persisted project trust.
    pub trust_root: String,
}

/// TS: git.ts WorktreeEntry (one `git worktree list --porcelain` record, bare ones included).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WorktreeEntry {
    pub path: String,
    pub branch: String,
    pub head: String,
    pub bare: bool,
    pub locked: bool,
    pub prunable: bool,
}

/// TS: git.ts findRepoRoot (`git rev-parse --show-toplevel`; `cwd` default the process cwd).
pub async fn find_repo_root(cwd: Option<&str>) -> std::result::Result<String, ExecError> {
    let _ = cwd;
    todo!("I3")
}

/// TS: git.ts ensureGitRepo (`deckhand must be run from inside a git repository`).
pub async fn ensure_git_repo(cwd: Option<&str>) -> Result<String> {
    let _ = cwd;
    todo!("I3")
}

/// TS: git.ts findGitCommonDir (absolute).
pub async fn find_git_common_dir(cwd: &str) -> std::result::Result<String, ExecError> {
    let _ = cwd;
    todo!("I3")
}

/// TS: git.ts resolveRepoContext (one rev-parse; the main checkout from the worktree list when `cwd` is a linked
/// worktree; trust keyed by the main checkout, or the canonical common dir of a bare repository).
pub async fn resolve_repo_context(cwd: &str) -> Result<RepoContext> {
    let _ = cwd;
    todo!("I3")
}

/// The parser of TS: git.ts worktreeEntries (`git worktree list --porcelain` output).
pub fn parse_worktree_list(stdout: &str) -> Vec<WorktreeEntry> {
    let _ = stdout;
    todo!("I3")
}

/// TS: git.ts worktreeEntries.
pub async fn worktree_entries(cwd: &str) -> Result<Vec<WorktreeEntry>> {
    let _ = cwd;
    todo!("I3")
}

/// TS: git.ts listWorktrees (checkouts only; the first entry is the main worktree).
pub async fn list_worktrees(cwd: &str) -> Result<Vec<WorktreeInfo>> {
    let _ = cwd;
    todo!("I3")
}

/// TS: git.ts sanitizeWorktreeName (lowercase `[a-z0-9_-/]`, runs collapsed, at most 96, `worktree` when empty).
pub fn sanitize_worktree_name(title: &str) -> String {
    let _ = title;
    todo!("I3")
}

/// TS: git.ts resolveCreateScript (the current checkout's `.claude/scripts/create-worktree.sh`, else the main
/// checkout's; regular files only).
pub async fn resolve_create_script(current_worktree_root: &str, main_worktree_root: Option<&str>) -> Option<String> {
    let _ = (current_worktree_root, main_worktree_root);
    todo!("I3")
}

/// TS: git.ts runCreateScript: runs the reviewed hook's exact bytes (`bash -c <content> <file>`) in `cwd` with
/// `CLAUDE_PROJECT_DIR=<launch_cwd>` and `{name, cwd}` JSON on stdin, 60 s at most; its last stdout line must be an
/// absolute path.
pub async fn run_create_script(hook: &CreationHook, name: &str, cwd: &str, launch_cwd: &str) -> Result<String> {
    let _ = (hook, name, cwd, launch_cwd);
    todo!("I3")
}

/// TS: git.ts resolveDefaultBranch `where`: which refs must exist.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BranchWhere {
    /// Local branches (`branchFrom: "default"`).
    #[default]
    Local,
    /// The remote's branches (before fetching for `branchFrom: "origin"`).
    Remote,
}

/// TS: git.ts resolveDefaultBranch (origin/HEAD's target, else main, else master; `remote` default `origin`).
pub async fn resolve_default_branch(cwd: &str, remote: Option<&str>, place: BranchWhere) -> Option<String> {
    let _ = (cwd, remote, place);
    todo!("I3")
}

/// TS: git.ts branchStart result (also chosenStart's): the commit a new branch starts at and the base it names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BranchStart {
    pub start: String,
    pub base_ref: String,
}

/// TS: git.ts branchStart (`from` default `current`: the launch checkout's HEAD; `default`: the local default
/// branch; `origin`: a freshly fetched origin default branch, never a stale ref).
pub async fn branch_start(root: &str, launch_cwd: &str, from: Option<BranchFrom>) -> Result<BranchStart> {
    let _ = (root, launch_cwd, from);
    todo!("I3")
}

/// TS: git.ts listLocalBranches (most recently committed first; `limit` default 200).
pub async fn list_local_branches(cwd: &str, limit: Option<usize>) -> Vec<String> {
    let _ = (cwd, limit);
    todo!("I3")
}

/// TS: git.ts chosenStart (a base chosen in the new-session form: a local branch, or `origin/<name>` as last
/// fetched).
pub async fn chosen_start(root: &str, base: &str) -> Result<BranchStart> {
    let _ = (root, base);
    todo!("I3")
}

/// TS: git.ts worktreeBranchName (`template` default `{name}`; checked with `git check-ref-format --branch`).
pub async fn worktree_branch_name(cwd: &str, name: &str, template: Option<&str>) -> Result<String> {
    let _ = (cwd, name, template);
    todo!("I3")
}

/// TS: git.ts fallbackCreateWorktree result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FallbackWorktree {
    pub path: String,
    pub base_ref: Option<String>,
}

/// TS: git.ts fallbackCreateWorktree (reuses an existing directory or branch; otherwise `git worktree add --no-track
/// -b <branch> <path> <start>`).
pub async fn fallback_create_worktree(
    worktree_path: &str,
    name: &str,
    current_worktree_root: &str,
    launch_cwd: &str,
    settings: Option<&WorktreeSettings>,
    base_branch: Option<&str>,
) -> Result<FallbackWorktree> {
    let _ = (
        worktree_path,
        name,
        current_worktree_root,
        launch_cwd,
        settings,
        base_branch,
    );
    todo!("I3")
}

/// TS: git.ts worktreeTemplateVars (placeholders from the main checkout, or a bare repository's common dir).
pub async fn worktree_template_vars(name: &str, cwd: &str) -> Result<TemplateVars> {
    let _ = (name, cwd);
    todo!("I3")
}

/// TS: git.ts createWorktreeForSession. `creation_hook` must be the trusted, verified hook (exactly its bytes run);
/// otherwise `settings` place the worktree (default `<home>/worktrees/<name>`) and its branch; links apply only to a
/// newly created worktree. A hook cannot be combined with `base_branch`.
pub async fn create_worktree_for_session(
    title: &str,
    launch_cwd: &str,
    creation_hook: Option<&CreationHook>,
    settings: Option<&WorktreeSettings>,
    base_branch: Option<&str>,
) -> Result<CreatedWorktreeInfo> {
    let _ = (title, launch_cwd, creation_hook, settings, base_branch);
    todo!("I3")
}

/// TS: git.ts cleanupEmptyWorktreeParents (rmdir of now-empty parents under Deckhand's worktree root, and the
/// intermediate directories of a nested `name`).
pub async fn cleanup_empty_worktree_parents(worktree_path: &str, name: Option<&str>) {
    let _ = (worktree_path, name);
    todo!("I3")
}

/// TS: git.ts removeWorktree (`git worktree remove -f`, then leftovers, empty parents, `git worktree prune`).
pub async fn remove_worktree(worktree_path: &str, repo_cwd: &str, name: Option<&str>) -> Result<()> {
    let _ = (worktree_path, repo_cwd, name);
    todo!("I3")
}

/// TS: git.ts pruneWorktrees.
pub async fn prune_worktrees(repo_cwd: &str) -> Result<()> {
    let _ = repo_cwd;
    todo!("I3")
}

/// TS: git.ts deleteLocalBranch (`git branch -D`; refuses unnamed, main and master).
pub async fn delete_local_branch(repo_cwd: &str, branch: &str) -> Result<()> {
    let _ = (repo_cwd, branch);
    todo!("I3")
}

/// TS: git.ts currentBranch (empty when HEAD is detached).
pub async fn current_branch(cwd: &str) -> std::result::Result<String, ExecError> {
    let _ = cwd;
    todo!("I3")
}

/// TS: git.ts headSha.
pub async fn head_sha(cwd: &str) -> std::result::Result<String, ExecError> {
    let _ = cwd;
    todo!("I3")
}

/// TS: git.ts countCommitsToMerge (`HEAD..<source_ref>`).
pub async fn count_commits_to_merge(target_root: &str, source_ref: &str) -> Result<i64> {
    let _ = (target_root, source_ref);
    todo!("I3")
}

/// TS: git.ts unmergedFiles (repository-relative).
pub async fn unmerged_files(cwd: &str) -> std::result::Result<Vec<String>, ExecError> {
    let _ = cwd;
    todo!("I3")
}

/// TS: git.ts operationInProgress (merge, rebase, cherry-pick, revert, or unresolved conflicts; per worktree).
pub async fn operation_in_progress(cwd: &str) -> Result<Option<TargetOperation>> {
    let _ = cwd;
    todo!("I3")
}

/// TS: git.ts uncommittedFiles (changed or untracked, not ignored).
pub async fn uncommitted_files(cwd: &str) -> Result<i64> {
    let _ = cwd;
    todo!("I3")
}

/// TS: git.ts commitWorktreeChanges (`git add -A`, `git commit -m`; `None` when there is nothing to commit; a failed
/// commit: `Commit failed in the worktree, so nothing was merged:\n<last 12 lines>`).
pub async fn commit_worktree_changes(cwd: &str, message: &str) -> Result<Option<CommittedChanges>> {
    let _ = (cwd, message);
    todo!("I3")
}

/// TS: git.ts MergeOptions.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MergeOptions {
    /// Commit the source worktree's uncommitted changes with this message before merging.
    pub commit_first: Option<String>,
}

/// TS: git.ts mergeWorktreeIntoCurrent result (`WorktreeMergeResult & {targetRoot, targetHead?, indexClean?}`). The
/// result's literals order `committed` and `reason` per outcome, hence `Ordered`.
#[derive(Clone, Debug, PartialEq)]
pub struct MergeOutcome {
    pub result: Ordered<WorktreeMergeResult>,
    pub target_root: String,
    /// The target's HEAD before a conflicted merge (to undo a squash).
    pub target_head: Option<String>,
    /// Whether the target's index was clean before a conflicted merge.
    pub index_clean: Option<bool>,
}

/// TS: git.ts mergeWorktreeIntoCurrent (`git merge --no-commit --no-ff` or `--squash` of the worktree's branch into
/// the target checkout; nothing new: skipped; conflicts: returned, not thrown).
pub async fn merge_worktree_into_current(
    worktree_path: &str,
    target_cwd: &str,
    mode: WorktreeMergeMode,
    options: &MergeOptions,
) -> Result<MergeOutcome> {
    let _ = (worktree_path, target_cwd, mode, options);
    todo!("I3")
}

/// TS: git.ts ConflictedMerge (what Deckhand knows about a conflicted merge it started).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConflictedMerge {
    pub mode: WorktreeMergeMode,
    pub target_head: Option<String>,
    pub index_clean: Option<bool>,
}

/// TS: git.ts abortMerge (`git merge --abort` with MERGE_HEAD; a squash only via `git reset --merge` when Deckhand
/// started it on a clean index and HEAD has not moved, else refused).
pub async fn abort_merge(target_root: &str, started: Option<&ConflictedMerge>) -> Result<()> {
    let _ = (target_root, started);
    todo!("I3")
}

/// TS: git.ts branchCreationCommit (the commit the branch was created at, from its reflog's oldest entry).
pub async fn branch_creation_commit(cwd: &str, branch: &str) -> Option<String> {
    let _ = (cwd, branch);
    todo!("I3")
}

/// TS: git.ts branchHasOwnCommits (the reflog shows a commit made on the branch; `None`: no reflog).
pub async fn branch_has_own_commits(cwd: &str, branch: &str) -> Option<bool> {
    let _ = (cwd, branch);
    todo!("I3")
}

/// TS: git.ts `output(error)`: an execFile error's stdout and stderr (trimmed), else its message.
pub fn error_output(error: &ExecError) -> String {
    let _ = error;
    todo!("I3")
}
