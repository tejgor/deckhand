//! Git plumbing for sessions and worktrees: bounded `git -C <cwd>` runs ([`git`], [`optional_git`]), repository
//! context, the worktree list, creating a session's worktree (a trusted creation hook, or Deckhand's own `git worktree
//! add` with worktree settings), removing and pruning worktrees, and merging a worktree into the launch checkout
//! without committing.
//!
//! Errors: Git failures are [`ExecError`]s (Node's execFile error: `Command failed: git -C ...\n<stderr>`, with
//! `stdout`/`stderr` kept, which callers fold into their messages); Deckhand's own refusals are [`DhError`]s with the
//! TS message. `ExecError` converts into `DhError` with `?`.

use std::collections::HashSet;
use std::sync::LazyLock;
use std::time::Duration;

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;

use crate::paths;
use crate::proc::{self, ExecError, ExecFailure, ExecOptions};
use crate::worktree_links::{LinkOptions, apply_worktree_links, user_slug};
use deckhand_core::error::{DhError, Result};
use deckhand_core::jsstr::{self, path};
use deckhand_core::worktree_links::{BranchVars, branch_name_problem, expand_branch_name, worktree_location};
use deckhand_protocol::data::{
    BranchFrom, CommittedChanges, CreationHook, TargetOperation, TemplateVars, WorktreeCreator, WorktreeLinks,
    WorktreeMergeMode, WorktreeMergeResult, WorktreeOrigin, WorktreeSettings,
};
use deckhand_protocol::{KeyOrder, Ordered};

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
    let defaults;
    let options = match options {
        Some(options) => options,
        None => {
            defaults = GitOptions::default();
            &defaults
        }
    };
    let mut exec_options = ExecOptions::default()
        .timeout(options.timeout)
        .max_buffer(options.max_buffer);
    for (key, value) in &options.env {
        exec_options = exec_options.env(key, value);
    }
    let mut full: Vec<&str> = Vec::with_capacity(args.len() + 2);
    full.push("-C");
    full.push(cwd);
    full.extend_from_slice(args);
    let output = proc::exec("git", &full, &exec_options).await?;
    Ok(GitOutput {
        stdout: output.stdout,
        stderr: output.stderr,
    })
}

/// TS: git.ts git (stdout with trailing whitespace trimmed).
pub async fn git(cwd: &str, args: &[&str], options: Option<&GitOptions>) -> std::result::Result<String, ExecError> {
    let output = git_output(cwd, args, options).await?;
    Ok(jsstr::js_trim_end(&output.stdout).to_owned())
}

/// TS: git.ts optionalGit (`None` on any failure).
pub async fn optional_git(cwd: &str, args: &[&str], options: Option<&GitOptions>) -> Option<String> {
    git(cwd, args, options).await.ok()
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

/// Node `fs.realpath(path)` as a string.
async fn realpath(target: &str) -> std::io::Result<String> {
    let real = tokio::fs::canonicalize(target).await?;
    Ok(real.to_string_lossy().into_owned())
}

/// `fs.realpath(path).catch(() => fallback)`.
async fn realpath_or(target: &str, fallback: String) -> String {
    realpath(target).await.unwrap_or(fallback)
}

/// `fs.access(path)` succeeded (follows symlinks; any error counts as absent).
async fn exists(target: &str) -> bool {
    !target.is_empty() && tokio::fs::metadata(target).await.is_ok()
}

/// TS: git.ts findRepoRoot (`git rev-parse --show-toplevel`; `cwd` default the process cwd).
pub async fn find_repo_root(cwd: Option<&str>) -> std::result::Result<String, ExecError> {
    let owned;
    let cwd = match cwd {
        Some(cwd) => cwd,
        None => {
            owned = paths::current_dir().to_string_lossy().into_owned();
            &owned
        }
    };
    git(cwd, &["rev-parse", "--show-toplevel"], None).await
}

/// TS: git.ts ensureGitRepo (`deckhand must be run from inside a git repository`).
pub async fn ensure_git_repo(cwd: Option<&str>) -> Result<String> {
    find_repo_root(cwd)
        .await
        .map_err(|_| DhError::msg("deckhand must be run from inside a git repository"))
}

/// TS: git.ts findGitCommonDir (absolute).
pub async fn find_git_common_dir(cwd: &str) -> std::result::Result<String, ExecError> {
    git(cwd, &["rev-parse", "--path-format=absolute", "--git-common-dir"], None).await
}

/// TS: git.ts resolveRepoContext (one rev-parse; the main checkout from the worktree list when `cwd` is a linked
/// worktree; trust keyed by the main checkout, or the canonical common dir of a bare repository).
pub async fn resolve_repo_context(cwd: &str) -> Result<RepoContext> {
    let stdout = git(
        cwd,
        &[
            "rev-parse",
            "--path-format=absolute",
            "--show-toplevel",
            "--git-dir",
            "--git-common-dir",
        ],
        None,
    )
    .await?;
    let mut lines = stdout.split('\n');
    let root = lines.next().unwrap_or_default().to_owned();
    let git_dir = lines.next().unwrap_or_default().to_owned();
    let common_dir = lines.next().unwrap_or_default().to_owned();
    let mut main_root = Some(root.clone());
    if path::resolve(&git_dir) != path::resolve(&common_dir) {
        let entries = worktree_entries(&root).await?;
        main_root = match entries.into_iter().next() {
            Some(main) if !main.bare => Some(realpath_or(&main.path, main.path.clone()).await),
            _ => None,
        };
    }
    // Trust stays keyed by the main checkout root so entries saved by earlier versions stay valid
    // (also correct for submodules, whose common dir lives in the superproject). Bare repositories
    // have no main checkout, so they use their canonical common dir, which is equally unique.
    let trust_root = match &main_root {
        Some(main_root) => main_root.clone(),
        None => realpath_or(&common_dir, path::resolve(&common_dir)).await,
    };
    Ok(RepoContext {
        root,
        common_dir,
        main_root,
        trust_root,
    })
}

/// The parser of TS: git.ts worktreeEntries (`git worktree list --porcelain` output).
pub fn parse_worktree_list(stdout: &str) -> Vec<WorktreeEntry> {
    let mut entries = Vec::new();
    let mut current: Option<WorktreeEntry> = None;
    let flush = |current: &mut Option<WorktreeEntry>, entries: &mut Vec<WorktreeEntry>| {
        if let Some(entry) = current.take()
            && !entry.path.is_empty()
        {
            entries.push(entry);
        }
    };
    for line in stdout.split('\n') {
        if let Some(rest) = line.strip_prefix("worktree ") {
            flush(&mut current, &mut entries);
            current = Some(WorktreeEntry {
                path: rest.to_owned(),
                ..WorktreeEntry::default()
            });
            continue;
        }
        let Some(entry) = current.as_mut() else {
            continue;
        };
        if let Some(rest) = line.strip_prefix("HEAD ") {
            entry.head = rest.to_owned();
        } else if line.starts_with("branch ") {
            // TS slices by the length of `branch refs/heads/`, whatever the ref is.
            entry.branch = jsstr::js_slice(line, "branch refs/heads/".len() as i64, None);
        } else if line == "bare" {
            entry.bare = true;
        } else if line == "locked" || line.starts_with("locked ") {
            entry.locked = true;
        } else if line == "prunable" || line.starts_with("prunable ") {
            // Git still lists it but its directory is gone.
            entry.prunable = true;
        } else if line.is_empty() {
            flush(&mut current, &mut entries);
        }
    }
    flush(&mut current, &mut entries);
    entries
}

/// TS: git.ts worktreeEntries.
pub async fn worktree_entries(cwd: &str) -> Result<Vec<WorktreeEntry>> {
    let stdout = git(cwd, &["worktree", "list", "--porcelain"], None).await?;
    Ok(parse_worktree_list(&stdout))
}

/// TS: git.ts listWorktrees (checkouts only; the first entry is the main worktree).
pub async fn list_worktrees(cwd: &str) -> Result<Vec<WorktreeInfo>> {
    // The first entry is the main worktree; a bare repository's first entry is not a checkout.
    Ok(worktree_entries(cwd)
        .await?
        .into_iter()
        .enumerate()
        .filter(|(_, entry)| !entry.bare)
        .map(|(index, entry)| WorktreeInfo {
            path: entry.path,
            branch: entry.branch,
            head: entry.head,
            is_main: index == 0,
            locked: entry.locked.then_some(true),
            prunable: entry.prunable.then_some(true),
        })
        .collect())
}

static NOT_NAME_CHARS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[^a-z0-9_\-/]+").expect("valid regex"));
static UNDERSCORE_RUN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"_+").expect("valid regex"));
static SLASH_RUN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"/+").expect("valid regex"));

/// `.replace(/^[/_-]+|[/_-]+$/g, '')`.
fn trim_name_edges(text: &str) -> &str {
    text.trim_matches(['/', '_', '-'])
}

/// TS: git.ts sanitizeWorktreeName (lowercase `[a-z0-9_-/]`, runs collapsed, at most 96, `worktree` when empty).
pub fn sanitize_worktree_name(title: &str) -> String {
    let lower = jsstr::js_to_lower(title);
    let replaced = NOT_NAME_CHARS.replace_all(&lower, "_");
    let collapsed = UNDERSCORE_RUN.replace_all(&replaced, "_");
    let slashes = SLASH_RUN.replace_all(&collapsed, "/");
    let mut sanitized = trim_name_edges(&slashes).to_owned();
    if sanitized.is_empty() {
        sanitized = "worktree".to_owned();
    }
    // Only ASCII is left, so UTF-16 units and bytes agree.
    let cut = jsstr::js_slice(&sanitized, 0, Some(96));
    let trimmed = trim_name_edges(&cut);
    if trimmed.is_empty() {
        "worktree".to_owned()
    } else {
        trimmed.to_owned()
    }
}

/// `fs.stat(path)` is a regular file (symlinks followed).
async fn file_exists(file: &str) -> bool {
    tokio::fs::metadata(file)
        .await
        .map(|metadata| metadata.is_file())
        .unwrap_or(false)
}

/// TS: git.ts resolveCreateScript (the current checkout's `.claude/scripts/create-worktree.sh`, else the main
/// checkout's; regular files only).
pub async fn resolve_create_script(current_worktree_root: &str, main_worktree_root: Option<&str>) -> Option<String> {
    let mut candidates = vec![path::join(&[current_worktree_root, CREATE_WORKTREE_SCRIPT])];
    if let Some(main) = main_worktree_root.filter(|main| !main.is_empty()) {
        candidates.push(path::join(&[main, CREATE_WORKTREE_SCRIPT]));
    }
    let mut seen = HashSet::new();
    for candidate in candidates {
        let resolved = path::resolve(&candidate);
        if !seen.insert(resolved.clone()) {
            continue;
        }
        if file_exists(&resolved).await {
            return Some(resolved);
        }
    }
    None
}

/// TS: git.ts runCreateScript: runs the reviewed hook's exact bytes (`bash -c <content> <file>`) in `cwd` with
/// `CLAUDE_PROJECT_DIR=<launch_cwd>` and `{name, cwd}` JSON on stdin, 60 s at most; its last stdout line must be an
/// absolute path.
pub async fn run_create_script(hook: &CreationHook, name: &str, cwd: &str, launch_cwd: &str) -> Result<String> {
    let input = format!(
        "{{\"name\":{},\"cwd\":{}}}\n",
        jsstr::json_stringify_str(name),
        jsstr::json_stringify_str(launch_cwd)
    );
    let options = ExecOptions::default()
        .cwd(cwd)
        .env("CLAUDE_PROJECT_DIR", launch_cwd)
        .stdin(input)
        .max_buffer(usize::MAX);
    // Run the verified in-memory bytes; never let bash re-read a file that may have changed. $0 stays the script path.
    let args = ["-c", hook.content.as_str(), hook.file.as_str()];
    // Like the TS timer: the run is abandoned (and its process group signalled) at 60 s, whatever it does then.
    let run = tokio::time::timeout(Duration::from_secs(60), proc::exec("bash", &args, &options)).await;
    let stdout = match run {
        Err(_) => return Err(DhError::msg("create-worktree.sh timed out after 60s")),
        Ok(Ok(output)) => output.stdout,
        Ok(Err(error)) => {
            let code = match &error.failure {
                ExecFailure::Exit { code } => code.to_string(),
                ExecFailure::Signal { .. } => "null".to_owned(),
                ExecFailure::Spawn { .. } | ExecFailure::MaxBuffer { .. } => return Err(error.into()),
            };
            return Err(DhError::msg(format!(
                "create-worktree.sh failed with exit code {code}: {}",
                jsstr::js_trim(&error.stderr)
            )));
        }
    };
    let returned = stdout
        .split('\n')
        .map(jsstr::js_trim)
        .rfind(|line| !line.is_empty())
        .map(str::to_owned);
    let Some(returned) = returned else {
        return Err(DhError::msg("create-worktree.sh did not print a worktree path"));
    };
    if !path::is_absolute(&returned) {
        return Err(DhError::msg(format!(
            "create-worktree.sh returned a non-absolute path: {returned}"
        )));
    }
    Ok(returned)
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
    let remote = remote.unwrap_or("origin");
    let head = optional_git(
        cwd,
        &["symbolic-ref", "--quiet", &format!("refs/remotes/{remote}/HEAD")],
        None,
    )
    .await
    .map(|head| head.replacen(&format!("refs/remotes/{remote}/"), "", 1));
    let mut names: Vec<String> = Vec::new();
    if let Some(head) = head.as_ref().filter(|head| !head.is_empty()) {
        names.push(head.clone());
    }
    names.push("main".to_owned());
    names.push("master".to_owned());
    for name in names {
        let reference = match place {
            BranchWhere::Local => format!("refs/heads/{name}"),
            BranchWhere::Remote => format!("refs/remotes/{remote}/{name}"),
        };
        if optional_git(cwd, &["show-ref", "--verify", "--quiet", &reference], None)
            .await
            .is_some()
        {
            return Some(name);
        }
    }
    // Never fetched: the remote may still have it, so let the fetch decide.
    match place {
        BranchWhere::Remote => Some(head.unwrap_or_else(|| "main".to_owned())),
        BranchWhere::Local => None,
    }
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
    match from.unwrap_or(BranchFrom::Current) {
        BranchFrom::Current => {
            let start = head_sha(launch_cwd).await?;
            let branch = current_branch(launch_cwd).await?;
            let base_ref = if branch.is_empty() { start.clone() } else { branch };
            Ok(BranchStart { start, base_ref })
        }
        BranchFrom::Default => {
            let Some(name) = resolve_default_branch(root, None, BranchWhere::Local).await else {
                return Err(DhError::msg(
                    "worktree.branchFrom is \"default\", but the repository has no local default branch (origin/HEAD, main or master)",
                ));
            };
            let start = git(
                root,
                &["rev-parse", "--verify", &format!("refs/heads/{name}^{{commit}}")],
                None,
            )
            .await?;
            Ok(BranchStart { start, base_ref: name })
        }
        BranchFrom::Origin => {
            let remote = "origin";
            let has_remote = optional_git(root, &["remote"], None)
                .await
                .is_some_and(|remotes| remotes.split('\n').any(|line| line == remote));
            if !has_remote {
                return Err(DhError::msg(
                    "worktree.branchFrom is \"origin\", but the repository has no origin remote",
                ));
            }
            let name = resolve_default_branch(root, Some(remote), BranchWhere::Remote)
                .await
                .unwrap_or_default();
            if name.is_empty() || name.starts_with('-') || branch_name_problem(&name).is_some() {
                return Err(DhError::msg(format!("Cannot determine {remote}'s default branch")));
            }
            // Never fall back to a stale ref: if the fetch fails, creation fails.
            let mut options = GitOptions::default().with_timeout(Duration::from_secs(60));
            for (key, value) in NON_INTERACTIVE_GIT {
                options = options.with_env(key, value);
            }
            let refspec = format!("+refs/heads/{name}:refs/remotes/{remote}/{name}");
            if let Err(error) = git(root, &["fetch", "--no-tags", remote, &refspec], Some(&options)).await {
                let stderr = jsstr::js_trim(&error.stderr);
                let text = if stderr.is_empty() {
                    error.message()
                } else {
                    stderr.to_owned()
                };
                let lines: Vec<&str> = text.split('\n').filter(|line| !line.is_empty()).collect();
                let tail = lines[lines.len().saturating_sub(2)..].join(" ");
                return Err(DhError::msg(format!(
                    "git fetch {remote} {name} failed, so the worktree was not created: {}",
                    jsstr::js_slice(&tail, 0, Some(300))
                )));
            }
            let start = git(
                root,
                &[
                    "rev-parse",
                    "--verify",
                    &format!("refs/remotes/{remote}/{name}^{{commit}}"),
                ],
                None,
            )
            .await?;
            Ok(BranchStart {
                start,
                base_ref: format!("{remote}/{name}"),
            })
        }
    }
}

/// TS: git.ts listLocalBranches (most recently committed first; `limit` default 200).
pub async fn list_local_branches(cwd: &str, limit: Option<usize>) -> Vec<String> {
    let count = format!("--count={}", limit.unwrap_or(200));
    let out = optional_git(
        cwd,
        &[
            "for-each-ref",
            "--sort=-committerdate",
            &count,
            "--format=%(refname:short)",
            "refs/heads",
        ],
        None,
    )
    .await;
    out.unwrap_or_default()
        .split('\n')
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect()
}

/// TS: git.ts chosenStart (a base chosen in the new-session form: a local branch, or `origin/<name>` as last
/// fetched).
pub async fn chosen_start(root: &str, base: &str) -> Result<BranchStart> {
    if base.is_empty() || base.starts_with('-') || branch_name_problem(base).is_some() {
        return Err(DhError::msg(format!(
            "Invalid base branch {}",
            jsstr::json_stringify_str(base)
        )));
    }
    let local = optional_git(
        root,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/heads/{base}^{{commit}}"),
        ],
        None,
    )
    .await
    .filter(|sha| !sha.is_empty());
    if let Some(start) = local {
        return Ok(BranchStart {
            start,
            base_ref: base.to_owned(),
        });
    }
    let remote = if base.starts_with("origin/") {
        optional_git(
            root,
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("refs/remotes/{base}^{{commit}}"),
            ],
            None,
        )
        .await
        .filter(|sha| !sha.is_empty())
    } else {
        None
    };
    if let Some(start) = remote {
        return Ok(BranchStart {
            start,
            base_ref: base.to_owned(),
        });
    }
    Err(DhError::msg(format!("Base branch {base} does not exist here")))
}

/// TS: git.ts worktreeBranchName (`template` default `{name}`; checked with `git check-ref-format --branch`).
pub async fn worktree_branch_name(cwd: &str, name: &str, template: Option<&str>) -> Result<String> {
    let template = template.unwrap_or("{name}");
    // TS reads the OS user eagerly; it only matters where the template uses it.
    let user = if template.contains("{user}") {
        user_slug()
    } else {
        String::new()
    };
    let branch = expand_branch_name(template, &BranchVars { name, user: &user })?;
    let checked = if branch_name_problem(&branch).is_some() {
        None
    } else {
        optional_git(cwd, &["check-ref-format", "--branch", &branch], None).await
    };
    if checked.as_deref() != Some(branch.as_str()) {
        return Err(DhError::msg(format!(
            "Invalid branch name {} from worktree.branchName {}",
            jsstr::json_stringify_str(&branch),
            jsstr::json_stringify_str(template)
        )));
    }
    Ok(branch)
}

/// TS: git.ts fallbackCreateWorktree result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FallbackWorktree {
    pub path: String,
    pub base_ref: Option<String>,
}

/// `fs.mkdir(path.dirname(file), {recursive: true})`.
async fn mkdir_parent(file: &str) -> Result<()> {
    let parent = path::dirname(file);
    tokio::fs::create_dir_all(&parent)
        .await
        .map_err(|error| DhError::io(&error, "mkdir", &parent))
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
    let template = settings.and_then(|settings| settings.branch_name.as_deref());
    let branch = worktree_branch_name(current_worktree_root, name, template).await?;
    if exists(worktree_path).await {
        return Ok(FallbackWorktree {
            path: worktree_path.to_owned(),
            base_ref: None,
        });
    }
    let slow = GitOptions::slow();
    let branch_ref = format!("refs/heads/{branch}");
    if optional_git(
        current_worktree_root,
        &["show-ref", "--verify", "--quiet", &branch_ref],
        None,
    )
    .await
    .is_some()
    {
        mkdir_parent(worktree_path).await?;
        git(
            current_worktree_root,
            &["worktree", "add", worktree_path, &branch],
            Some(&slow),
        )
        .await?;
        return Ok(FallbackWorktree {
            path: worktree_path.to_owned(),
            base_ref: None,
        });
    }
    let BranchStart { start, base_ref } = match base_branch.filter(|base| !base.is_empty()) {
        Some(base) => chosen_start(current_worktree_root, base).await?,
        None => {
            let from = settings.and_then(|settings| settings.branch_from);
            branch_start(current_worktree_root, launch_cwd, from).await?
        }
    };
    mkdir_parent(worktree_path).await?;
    // --no-track: a remote start point must not become the branch's upstream (create-pr sets that when pushing).
    git(
        current_worktree_root,
        &["worktree", "add", "--no-track", "-b", &branch, worktree_path, &start],
        Some(&slow),
    )
    .await?;
    Ok(FallbackWorktree {
        path: worktree_path.to_owned(),
        base_ref: Some(base_ref),
    })
}

/// `.replace(/\.git$/, '')`.
fn strip_git_suffix(name: &str) -> &str {
    name.strip_suffix(".git").unwrap_or(name)
}

/// TS: git.ts worktreeTemplateVars (placeholders from the main checkout, or a bare repository's common dir).
pub async fn worktree_template_vars(name: &str, cwd: &str) -> Result<TemplateVars> {
    let RepoContext {
        main_root, common_dir, ..
    } = resolve_repo_context(cwd).await?;
    let repo_root = match main_root {
        Some(main_root) => main_root,
        None => realpath_or(&common_dir, path::resolve(&common_dir)).await,
    };
    let base = path::basename(&repo_root);
    let repo = strip_git_suffix(&base);
    Ok(TemplateVars {
        name: name.to_owned(),
        repo: if repo.is_empty() {
            "repo".to_owned()
        } else {
            repo.to_owned()
        },
        repo_parent: path::dirname(&repo_root),
        repo_root,
        home: paths::home_dir().to_string_lossy().into_owned(),
    })
}

fn has_links(settings: Option<&WorktreeSettings>) -> bool {
    settings.is_some_and(|settings| {
        settings.symlink.as_ref().is_some_and(|symlink| !symlink.is_empty())
            || settings.files.as_ref().is_some_and(|files| !files.is_empty())
    })
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
    let base_branch = base_branch.filter(|base| !base.is_empty());
    if creation_hook.is_some() && base_branch.is_some() {
        return Err(DhError::msg(
            "This repository's worktree creation hook decides where new worktrees start, so a base branch cannot be chosen; leave Base on its default",
        ));
    }
    let name = sanitize_worktree_name(title);
    let current_worktree_root = find_repo_root(Some(launch_cwd)).await?;
    let before: HashSet<String> = list_worktrees(&current_worktree_root)
        .await?
        .iter()
        .map(|worktree| path::resolve(&worktree.path))
        .collect();
    let creator = if creation_hook.is_some() {
        WorktreeCreator::Script
    } else {
        WorktreeCreator::Fallback
    };
    let location = settings
        .and_then(|settings| settings.location.as_deref())
        .filter(|location| !location.is_empty());
    let vars = if location.is_some() || has_links(settings) {
        Some(worktree_template_vars(&name, &current_worktree_root).await?)
    } else {
        None
    };
    let (worktree_path, base_ref) = match creation_hook {
        Some(hook) => (
            run_create_script(hook, &name, &current_worktree_root, launch_cwd).await?,
            None,
        ),
        None => {
            let target = match (location, &vars) {
                (Some(location), Some(vars)) => worktree_location(location, vars)?,
                _ => path::join(&[&paths::config_dir().to_string_lossy(), "worktrees", &name]),
            };
            let created = fallback_create_worktree(
                &target,
                &name,
                &current_worktree_root,
                launch_cwd,
                settings,
                base_branch,
            )
            .await?;
            (created.path, created.base_ref)
        }
    };
    let resolved = path::resolve(&worktree_path);
    let absolute_path = realpath(&resolved)
        .await
        .map_err(|error| DhError::io(&error, "realpath", &resolved))?;
    let after = list_worktrees(&current_worktree_root).await?;
    let Some(metadata) = after
        .into_iter()
        .find(|worktree| path::resolve(&worktree.path) == absolute_path)
    else {
        return Err(DhError::msg(format!(
            "created path is not registered as a git worktree: {absolute_path}"
        )));
    };
    let origin = if before.contains(&absolute_path) {
        WorktreeOrigin::Existing
    } else {
        WorktreeOrigin::Created
    };
    let links = match &vars {
        Some(vars) if origin == WorktreeOrigin::Created && has_links(settings) => {
            let options = LinkOptions {
                launch_root: current_worktree_root.clone(),
                worktree_root: absolute_path.clone(),
                vars: vars.clone(),
            };
            Some(apply_worktree_links(settings, &options).await)
        }
        _ => None,
    };
    let base_ref = base_ref.filter(|base_ref| !base_ref.is_empty() && origin == WorktreeOrigin::Created);
    Ok(CreatedWorktreeInfo {
        path: absolute_path,
        branch: metadata.branch,
        head: metadata.head,
        is_main: metadata.is_main,
        locked: metadata.locked,
        prunable: metadata.prunable,
        origin,
        creator,
        name,
        links,
        base_ref,
    })
}

/// TS: git.ts cleanupEmptyWorktreeParents (rmdir of now-empty parents under Deckhand's worktree root, and the
/// intermediate directories of a nested `name`).
pub async fn cleanup_empty_worktree_parents(worktree_path: &str, name: Option<&str>) {
    let config_dir = paths::config_dir().to_string_lossy().into_owned();
    let resolved_root = path::resolve_all(&[&config_dir, "worktrees"]);
    let worktrees_root = realpath_or(&resolved_root, resolved_root.clone()).await;
    let mut current = path::dirname(&path::resolve(worktree_path));
    let mut nested: i64 = match name.filter(|name| !name.is_empty()) {
        Some(name) if worktree_path.ends_with(&format!("{}{name}", path::SEP)) => name.split('/').count() as i64 - 1,
        _ => 0,
    };
    let prefix = format!("{worktrees_root}{}", path::SEP);
    loop {
        let under = current != worktrees_root && current.starts_with(&prefix);
        // `nested-- > 0` is only evaluated (and decremented) when `under` is false.
        let go = under || {
            let old = nested;
            nested -= 1;
            old > 0
        };
        if !go || tokio::fs::remove_dir(&current).await.is_err() {
            break;
        }
        current = path::dirname(&current);
    }
}

/// `fs.rm(target, {recursive: true, force: true, maxRetries: 3, retryDelay: 100})` (symlinks are removed, not
/// followed).
async fn remove_recursive(target: &str) -> Result<()> {
    let mut attempt: u64 = 0;
    loop {
        let result = match tokio::fs::symlink_metadata(target).await {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => Err(error),
            Ok(metadata) if metadata.is_dir() => tokio::fs::remove_dir_all(target).await,
            Ok(_) => tokio::fs::remove_file(target).await,
        };
        match result {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => {
                let retryable = matches!(
                    deckhand_core::error::errno_code(&error),
                    Some("EBUSY" | "EMFILE" | "ENFILE" | "ENOTEMPTY" | "EPERM")
                );
                if !retryable || attempt >= 3 {
                    return Err(DhError::io(&error, "rm", target));
                }
                attempt += 1;
                tokio::time::sleep(Duration::from_millis(100 * attempt)).await;
            }
        }
    }
}

/// TS: git.ts removeWorktree (`git worktree remove -f`, then leftovers, empty parents, `git worktree prune`).
pub async fn remove_worktree(worktree_path: &str, repo_cwd: &str, name: Option<&str>) -> Result<()> {
    let absolute_path = path::resolve(worktree_path);
    let slow = GitOptions::slow();
    git(repo_cwd, &["worktree", "remove", "-f", &absolute_path], Some(&slow)).await?;
    // Some hook-created or nested managed worktrees can leave ignored files, empty
    // directories, or parent folders behind after Git unregisters the worktree.
    remove_recursive(&absolute_path).await?;
    cleanup_empty_worktree_parents(&absolute_path, name).await;
    git(repo_cwd, &["worktree", "prune"], Some(&slow)).await?;
    Ok(())
}

/// TS: git.ts pruneWorktrees.
pub async fn prune_worktrees(repo_cwd: &str) -> Result<()> {
    git(repo_cwd, &["worktree", "prune"], Some(&GitOptions::slow())).await?;
    Ok(())
}

/// TS: git.ts deleteLocalBranch (`git branch -D`; refuses unnamed, main and master).
pub async fn delete_local_branch(repo_cwd: &str, branch: &str) -> Result<()> {
    let trimmed = jsstr::js_trim(branch);
    if trimmed.is_empty() {
        return Err(DhError::msg("cannot delete an unnamed branch"));
    }
    if trimmed == "main" || trimmed == "master" {
        return Err(DhError::msg(format!("refusing to delete protected branch {trimmed}")));
    }
    git(repo_cwd, &["branch", "-D", trimmed], None).await?;
    Ok(())
}

/// TS: git.ts currentBranch (empty when HEAD is detached).
pub async fn current_branch(cwd: &str) -> std::result::Result<String, ExecError> {
    git(cwd, &["branch", "--show-current"], None).await
}

/// TS: git.ts headSha.
pub async fn head_sha(cwd: &str) -> std::result::Result<String, ExecError> {
    git(cwd, &["rev-parse", "--verify", "HEAD"], None).await
}

/// `Number.parseInt(text, 10)`, `None` for NaN.
fn parse_int(text: &str) -> Option<i64> {
    let text = jsstr::js_trim_start(text);
    let (negative, digits) = match text.as_bytes().first() {
        Some(b'-') => (true, &text[1..]),
        Some(b'+') => (false, &text[1..]),
        _ => (false, text),
    };
    let end = digits.bytes().take_while(u8::is_ascii_digit).count();
    let value: i64 = digits[..end].parse().ok()?;
    Some(if negative { -value } else { value })
}

/// TS: git.ts countCommitsToMerge (`HEAD..<source_ref>`).
pub async fn count_commits_to_merge(target_root: &str, source_ref: &str) -> Result<i64> {
    let range = format!("HEAD..{source_ref}");
    let out = git(target_root, &["rev-list", "--count", &range], None).await?;
    Ok(parse_int(&out).unwrap_or(0))
}

/// TS: git.ts unmergedFiles (repository-relative).
pub async fn unmerged_files(cwd: &str) -> std::result::Result<Vec<String>, ExecError> {
    let out = git(cwd, &["diff", "--name-only", "-z", "--diff-filter=U"], None).await?;
    Ok(out
        .split('\0')
        .filter(|file| !file.is_empty())
        .map(str::to_owned)
        .collect())
}

/// TS: git.ts operationInProgress (merge, rebase, cherry-pick, revert, or unresolved conflicts; per worktree).
pub async fn operation_in_progress(cwd: &str) -> Result<Option<TargetOperation>> {
    let out = git(
        cwd,
        &[
            "rev-parse",
            "--path-format=absolute",
            "--git-path",
            "MERGE_HEAD",
            "--git-path",
            "rebase-merge",
            "--git-path",
            "rebase-apply",
            "--git-path",
            "CHERRY_PICK_HEAD",
            "--git-path",
            "REVERT_HEAD",
        ],
        None,
    )
    .await?;
    let mut lines = out.split('\n');
    let mut next = || lines.next().unwrap_or_default().to_owned();
    let (merge, rebase_merge, rebase_apply, cherry_pick, revert) = (next(), next(), next(), next(), next());
    if exists(&rebase_merge).await || exists(&rebase_apply).await {
        return Ok(Some(TargetOperation::Rebase));
    }
    if exists(&merge).await {
        return Ok(Some(TargetOperation::Merge));
    }
    if exists(&cherry_pick).await {
        return Ok(Some(TargetOperation::CherryPick));
    }
    if exists(&revert).await {
        return Ok(Some(TargetOperation::Revert));
    }
    Ok((!unmerged_files(cwd).await?.is_empty()).then_some(TargetOperation::Conflicts))
}

/// TS: git.ts operationProblem, the text merges refuse with. Same as [`operation_problem`]; this module keeps its own
/// copy so merges (and the git/operationProblem replay) do not wait on the merge-model port.
fn target_problem(operation: TargetOperation) -> String {
    match operation {
        TargetOperation::Conflicts => "The target has unresolved conflicts: resolve and commit them first".to_owned(),
        operation => format!("A {operation} is in progress in the target: finish or abort it first"),
    }
}

/// TS: git.ts uncommittedFiles (changed or untracked, not ignored).
pub async fn uncommitted_files(cwd: &str) -> Result<i64> {
    let options = GitOptions::default().with_max_buffer(32 * 1024 * 1024);
    let out = git(
        cwd,
        &[
            "--no-optional-locks",
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
        ],
        Some(&options),
    )
    .await?;
    let records: Vec<&str> = out.split('\0').collect();
    let mut files = 0;
    let mut i = 0;
    while i < records.len() {
        let record = records[i];
        if jsstr::js_len(record) >= 4 {
            files += 1;
            // A rename's or copy's original path follows.
            if record.starts_with(['R', 'C']) {
                i += 1;
            }
        }
        i += 1;
    }
    Ok(files)
}

/// TS: git.ts commitWorktreeChanges (`git add -A`, `git commit -m`; `None` when there is nothing to commit; a failed
/// commit: `Commit failed in the worktree, so nothing was merged:\n<last 12 lines>`).
pub async fn commit_worktree_changes(cwd: &str, message: &str) -> Result<Option<CommittedChanges>> {
    let files = uncommitted_files(cwd).await?;
    if files == 0 {
        return Ok(None);
    }
    let slow = GitOptions::slow();
    let commit = async {
        git(cwd, &["add", "-A"], Some(&slow)).await?;
        let options = GitOptions::slow().with_env("GIT_EDITOR", "true");
        git(cwd, &["commit", "--no-edit", "-m", message], Some(&options)).await
    };
    if let Err(error) = commit.await {
        let output = error_output(&error);
        let lines: Vec<&str> = output.split('\n').collect();
        let tail = lines[lines.len().saturating_sub(12)..].join("\n");
        return Err(DhError::msg(format!(
            "Commit failed in the worktree, so nothing was merged:\n{tail}"
        )));
    }
    Ok(Some(CommittedChanges {
        files,
        sha: head_sha(cwd).await?,
    }))
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

/// The key order of TS's skipped literal (`..., skipped, reason, committed?, stdout, stderr`).
fn skipped_key_order() -> KeyOrder {
    KeyOrder::Object(
        [
            "mode",
            "sourceRef",
            "sourceSha",
            "targetBranch",
            "skipped",
            "reason",
            "committed",
            "stdout",
            "stderr",
        ]
        .into_iter()
        .map(|key| (key.to_owned(), KeyOrder::Unrecorded))
        .collect(),
    )
}

/// TS: git.ts mergeWorktreeIntoCurrent (`git merge --no-commit --no-ff` or `--squash` of the worktree's branch into
/// the target checkout; nothing new: skipped; conflicts: returned, not thrown).
pub async fn merge_worktree_into_current(
    worktree_path: &str,
    target_cwd: &str,
    mode: WorktreeMergeMode,
    options: &MergeOptions,
) -> Result<MergeOutcome> {
    let source_root = path::resolve(&find_repo_root(Some(worktree_path)).await?);
    let target_root = path::resolve(&find_repo_root(Some(target_cwd)).await?);
    if source_root == target_root {
        return Err(DhError::msg("cannot merge a worktree into itself"));
    }

    let source_branch = current_branch(&source_root).await?;
    let target_branch = current_branch(&target_root).await?;
    if target_branch.is_empty() {
        return Err(DhError::msg(
            "target worktree is detached; checkout a branch before merging",
        ));
    }
    if let Some(operation) = operation_in_progress(&target_root).await? {
        return Err(DhError::msg(target_problem(operation)));
    }
    let committed = match options.commit_first.as_deref().filter(|message| !message.is_empty()) {
        Some(message) => commit_worktree_changes(&source_root, message).await?,
        None => None,
    };
    let source_ref = if source_branch.is_empty() {
        head_sha(&source_root).await?
    } else {
        source_branch
    };
    // Recorded with the marker: the branch moves on, the commit does not (squash merges leave the branch "unmerged").
    let source_sha = git(
        &source_root,
        &["rev-parse", "--verify", &format!("{source_ref}^{{commit}}")],
        None,
    )
    .await?;

    let base = WorktreeMergeResult {
        mode,
        source_ref: source_ref.clone(),
        source_sha: Some(source_sha.clone()),
        target_branch,
        skipped: None,
        conflicted: None,
        conflicts: None,
        conflict_count: None,
        committed,
        reason: None,
        stdout: String::new(),
        stderr: String::new(),
        sent_items: None,
    };

    if count_commits_to_merge(&target_root, &source_sha).await? == 0 {
        let result = WorktreeMergeResult {
            skipped: Some(true),
            reason: Some("No new commits to merge".to_owned()),
            ..base
        };
        return Ok(MergeOutcome {
            result: Ordered::with_order(result, skipped_key_order()),
            target_root,
            target_head: None,
            index_clean: None,
        });
    }

    // A conflicted squash has no MERGE_HEAD; it can be undone (`git reset --merge`) only when the index was clean before,
    // which ort guarantees for a merge it started (it refuses with any staged change). Checked anyway.
    let (target_head, index_clean) = tokio::join!(
        head_sha(&target_root),
        optional_git(&target_root, &["diff", "--cached", "--quiet"], None)
    );
    let target_head = target_head?;
    let index_clean = index_clean.is_some();
    let args: &[&str] = match mode {
        WorktreeMergeMode::Squash => &["merge", "--squash", &source_ref],
        WorktreeMergeMode::Merge => &["merge", "--no-commit", "--no-ff", &source_ref],
    };
    match git_output(&target_root, args, Some(&GitOptions::slow())).await {
        Ok(GitOutput { stdout, stderr }) => Ok(MergeOutcome {
            result: Ordered::new(WorktreeMergeResult { stdout, stderr, ..base }),
            target_root,
            target_head: None,
            index_clean: None,
        }),
        Err(error) => {
            let message = jsstr::js_trim(
                &[error.message(), error.stdout.clone(), error.stderr.clone()]
                    .into_iter()
                    .filter(|part| !part.is_empty())
                    .collect::<Vec<_>>()
                    .join("\n"),
            )
            .to_owned();
            let conflicts = unmerged_files(&target_root).await.unwrap_or_default();
            if !conflicts.is_empty() {
                let count = conflicts.len() as i64;
                let shown: Vec<String> = conflicts.into_iter().take(MAX_CONFLICT_PATHS).collect();
                let result = WorktreeMergeResult {
                    conflicted: Some(true),
                    conflicts: Some(shown),
                    conflict_count: Some(count),
                    reason: Some("Merge has conflicts to resolve".to_owned()),
                    stdout: error.stdout,
                    stderr: error.stderr,
                    ..base
                };
                return Ok(MergeOutcome {
                    result: Ordered::new(result),
                    target_root,
                    target_head: Some(target_head),
                    index_clean: Some(index_clean),
                });
            }
            if message.is_empty() {
                let what = match mode {
                    WorktreeMergeMode::Squash => "squash merge",
                    WorktreeMergeMode::Merge => "merge",
                };
                return Err(DhError::msg(format!("{what} failed")));
            }
            Err(DhError::msg(message))
        }
    }
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
    let slow = GitOptions::slow();
    let merge_head = git(
        target_root,
        &["rev-parse", "--path-format=absolute", "--git-path", "MERGE_HEAD"],
        None,
    )
    .await?;
    if exists(&merge_head).await {
        git(target_root, &["merge", "--abort"], Some(&slow)).await?;
        return Ok(());
    }
    let safe = match started {
        Some(ConflictedMerge {
            mode: WorktreeMergeMode::Squash,
            target_head: Some(target_head),
            index_clean: Some(true),
        }) if !target_head.is_empty() => head_sha(target_root).await.ok().as_deref() == Some(target_head.as_str()),
        _ => false,
    };
    if !safe {
        return Err(DhError::msg(
            "Cannot abort this squash merge safely (Deckhand did not start it on a clean index, or the target changed since); undo it in Git",
        ));
    }
    git(target_root, &["reset", "--merge"], Some(&slow)).await?;
    let squash_msg = git(
        target_root,
        &["rev-parse", "--path-format=absolute", "--git-path", "SQUASH_MSG"],
        None,
    )
    .await?;
    remove_recursive_force(&squash_msg).await
}

/// `fs.rm(file, {force: true})` (a missing file is fine).
async fn remove_recursive_force(file: &str) -> Result<()> {
    match tokio::fs::remove_file(file).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(DhError::io(&error, "rm", file)),
    }
}

static CREATED_FROM: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^([0-9a-f]{40,64}) branch: Created from ").expect("valid regex"));

/// TS: git.ts branchCreationCommit (the commit the branch was created at, from its reflog's oldest entry).
pub async fn branch_creation_commit(cwd: &str, branch: &str) -> Option<String> {
    if branch.is_empty() || branch.starts_with('-') {
        return None;
    }
    let reference = format!("refs/heads/{branch}");
    let out = optional_git(cwd, &["log", "-g", "--format=%H %gs", &reference, "--"], None).await?;
    let oldest = out.split('\n').rfind(|line| !line.is_empty())?;
    CREATED_FROM
        .captures(oldest)
        .and_then(|captures| captures.get(1))
        .map(|sha| sha.as_str().to_owned())
}

/// `/^(commit|cherry-pick)\b/` and `/^(merge|pull)\b.*: Merge made by/` (JS `\b` is ASCII, `.` stops at line
/// terminators).
static OWN_COMMIT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:(?:commit|cherry-pick)(?-u:\b)|(?:merge|pull)(?-u:\b)[^\n\r\x{2028}\x{2029}]*: Merge made by)")
        .expect("valid regex")
});

/// TS: git.ts branchHasOwnCommits (the reflog shows a commit made on the branch; `None`: no reflog).
pub async fn branch_has_own_commits(cwd: &str, branch: &str) -> Option<bool> {
    let reference = format!("refs/heads/{branch}");
    let subjects = optional_git(cwd, &["log", "-g", "--format=%gs", &reference, "--"], None).await?;
    if jsstr::js_trim(&subjects).is_empty() {
        return None;
    }
    // A fast-forward, reset or rebase moves the branch without a commit of its own (e.g. a fresh branch brought up to date).
    Some(subjects.split('\n').any(|subject| OWN_COMMIT.is_match(subject)))
}

/// TS: git.ts `output(error)`: an execFile error's stdout and stderr (trimmed), else its message.
pub fn error_output(error: &ExecError) -> String {
    proc::failure_output(error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use deckhand_testkit::{Case, DirOp, Outcome, dir_cases, replay, replay_dir, tags};
    use std::path::Path;
    use std::process::Command;

    // Merging a worktree (m) against real fixture repositories, as tests/merge.test.ts does. The helpers' own Git
    // runs ignore the user's Git config; library calls rely on the repository's local config.

    fn run_git(dir: &str, args: &[&str]) -> std::result::Result<String, String> {
        let output = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .map_err(|error| error.to_string())?;
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        if output.status.success() {
            Ok(stdout)
        } else {
            Err(format!("{stdout}{}", String::from_utf8_lossy(&output.stderr)))
        }
    }

    /// tests/helpers.ts `git(cwd, ...args)`: trimmed stdout, panics on failure.
    fn tgit(dir: &str, args: &[&str]) -> String {
        match run_git(dir, args) {
            Ok(stdout) => stdout.trim_end().to_owned(),
            Err(error) => panic!("git {args:?} failed: {error}"),
        }
    }

    /// Untrimmed at the start: the first line's leading status column matters.
    fn status(dir: &str) -> String {
        run_git(dir, &["status", "--porcelain", "--untracked-files=all"])
            .unwrap()
            .trim_end()
            .to_owned()
    }

    fn sorted_lines(text: &str) -> Vec<String> {
        let mut lines: Vec<String> = text.split('\n').map(str::to_owned).collect();
        lines.sort();
        lines
    }

    fn real(dir: &Path) -> String {
        std::fs::canonicalize(dir).unwrap().to_string_lossy().into_owned()
    }

    fn write(dir: &str, file: &str, text: &str) {
        std::fs::write(Path::new(dir).join(file), text).unwrap();
    }

    fn read(dir: &str, file: &str) -> String {
        std::fs::read_to_string(Path::new(dir).join(file)).unwrap()
    }

    fn commit(dir: &str, file: &str, text: &str, message: &str) {
        write(dir, file, text);
        tgit(dir, &["add", "-A"]);
        tgit(dir, &["commit", "-m", message]);
    }

    /// tests/helpers.ts `repo()`.
    fn repo() -> tempfile::TempDir {
        let dir = tempfile::Builder::new().prefix("deckhand-test-").tempdir().unwrap();
        let path = real(dir.path());
        tgit(&path, &["init", "-b", "main"]);
        tgit(&path, &["config", "user.name", "Deckhand Test"]);
        tgit(&path, &["config", "user.email", "deckhand@example.invalid"]);
        tgit(&path, &["config", "commit.gpgsign", "false"]);
        let no_hooks = Path::new(&path).join(".git").join("no-hooks");
        tgit(&path, &["config", "core.hooksPath", &no_hooks.to_string_lossy()]);
        write(&path, "file.txt", "first\n");
        tgit(&path, &["add", "."]);
        tgit(&path, &["commit", "-m", "initial"]);
        dir
    }

    struct Fixture {
        _root_dir: tempfile::TempDir,
        parent: tempfile::TempDir,
        root: String,
    }

    impl Fixture {
        fn new() -> Self {
            let root_dir = repo();
            let root = real(root_dir.path());
            let parent = tempfile::Builder::new().prefix("deckhand-merge-wt-").tempdir().unwrap();
            write(&root, "other.txt", "other\n");
            tgit(&root, &["add", "."]);
            tgit(&root, &["commit", "-m", "other"]);
            Fixture {
                _root_dir: root_dir,
                parent,
                root,
            }
        }

        fn worktree(&self, name: &str) -> String {
            let dir = self.parent.path().join(name);
            tgit(&self.root, &["worktree", "add", "-b", name, &dir.to_string_lossy()]);
            real(&dir)
        }
    }

    fn commit_first(message: &str) -> MergeOptions {
        MergeOptions {
            commit_first: Some(message.to_owned()),
        }
    }

    fn set_executable(file: &Path) {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = std::fs::metadata(file).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(file, permissions).unwrap();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn preview_commit_first_commits_before_merging() {
        let fx = Fixture::new();
        let root = fx.root.as_str();
        let wt = fx.worktree("feat");
        commit(&wt, "a.txt", "one\ntwo\nthree\n", "add a");
        commit(&wt, "file.txt", "first\nchanged\n", "change file");
        write(&wt, "other.txt", "other edited\n");
        write(&wt, "new.txt", "untracked\n");
        // The git.ts inputs of the preview. getMergePreview itself (commits, diff stat, overlap, sourceSha) is
        // tested in workspace_git.rs.
        assert_eq!(uncommitted_files(&wt).await.unwrap(), 2);
        assert_eq!(count_commits_to_merge(root, "feat").await.unwrap(), 2);
        assert_eq!(current_branch(root).await.unwrap(), "main");
        assert_eq!(
            resolve_default_branch(root, None, BranchWhere::Local).await.as_deref(),
            Some("main")
        );
        assert_eq!(head_sha(&wt).await.unwrap(), tgit(&wt, &["rev-parse", "HEAD"]));
        // targetIsMain: the main checkout is its own main root; another linked worktree is not.
        let context = resolve_repo_context(root).await.unwrap();
        assert_eq!(
            (context.root.as_str(), context.main_root.as_deref()),
            (root, Some(root))
        );
        let other = fx.worktree("other-target");
        let context = resolve_repo_context(&other).await.unwrap();
        assert_eq!(context.root, other);
        assert_eq!(context.main_root.as_deref(), Some(root));
        assert_ne!(context.main_root.as_deref(), Some(other.as_str()));
        // Into itself: the source and target resolve to the same checkout.
        assert_eq!(find_repo_root(Some(&wt)).await.unwrap(), wt);
        let error = merge_worktree_into_current(&wt, &wt, WorktreeMergeMode::Merge, &MergeOptions::default())
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), "cannot merge a worktree into itself");

        let outcome = merge_worktree_into_current(&wt, root, WorktreeMergeMode::Squash, &commit_first("auth refactor"))
            .await
            .unwrap();
        let result = &outcome.result;
        assert!(result.skipped.is_none() && result.conflicted.is_none());
        assert_eq!(result.committed.as_ref().map(|committed| committed.files), Some(2));
        assert_eq!(tgit(&wt, &["log", "-1", "--format=%s"]), "auth refactor");
        assert_eq!(
            result.source_sha.as_deref(),
            Some(tgit(&wt, &["rev-parse", "HEAD"]).as_str())
        );
        assert_eq!(outcome.target_root, root);
        assert_eq!(status(&wt), "");
        // Everything, the committed-first files included, is staged in the target, uncommitted.
        assert_eq!(
            sorted_lines(&tgit(root, &["diff", "--cached", "--name-only"])),
            ["a.txt", "file.txt", "new.txt", "other.txt"]
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn commit_first_toggle_and_failing_hook() {
        let fx = Fixture::new();
        let root = fx.root.as_str();
        let wt = fx.worktree("only-edits");
        write(&wt, "file.txt", "edited\n");
        write(&wt, "fresh.txt", "fresh\n");
        assert_eq!(count_commits_to_merge(root, "only-edits").await.unwrap(), 0);
        assert_eq!(uncommitted_files(&wt).await.unwrap(), 2);
        let off = merge_worktree_into_current(&wt, root, WorktreeMergeMode::Merge, &MergeOptions::default())
            .await
            .unwrap();
        assert_eq!(off.result.skipped, Some(true));
        assert_eq!(off.result.reason.as_deref(), Some("No new commits to merge"));
        assert_eq!(off.result.committed, None);
        // The TS literal's order: reason before committed (targetRoot is kept outside the result).
        let keys: Vec<String> = match serde_json::to_value(&off.result).unwrap() {
            serde_json::Value::Object(map) => map.keys().cloned().collect(),
            _ => panic!("not an object"),
        };
        assert_eq!(
            keys,
            [
                "mode",
                "sourceRef",
                "sourceSha",
                "targetBranch",
                "skipped",
                "reason",
                "stdout",
                "stderr"
            ]
        );
        assert_eq!(sorted_lines(&status(&wt)), [" M file.txt", "?? fresh.txt"]);
        assert_eq!(status(root), "");
        let on = merge_worktree_into_current(&wt, root, WorktreeMergeMode::Merge, &commit_first("only edits"))
            .await
            .unwrap();
        assert!(on.result.skipped.is_none() && on.result.conflicted.is_none());
        assert_eq!(on.result.committed.as_ref().map(|committed| committed.files), Some(2));
        assert!(status(root).contains("fresh.txt"));
        tgit(root, &["merge", "--abort"]);

        // A pre-commit hook that fails: its output comes back, nothing is merged into the target.
        let hooks = Path::new(root).join(".git").join("failing-hooks");
        std::fs::create_dir(&hooks).unwrap();
        let hook = hooks.join("pre-commit");
        std::fs::write(&hook, "#!/bin/sh\necho \"lint: 3 problems\" >&2\nexit 1\n").unwrap();
        set_executable(&hook);
        tgit(root, &["config", "core.hooksPath", &hooks.to_string_lossy()]);
        let blocked = fx.worktree("hooked");
        write(&blocked, "blocked.txt", "x\n");
        let error = merge_worktree_into_current(&blocked, root, WorktreeMergeMode::Squash, &commit_first("blocked"))
            .await
            .unwrap_err()
            .to_string();
        assert!(
            error.starts_with("Commit failed in the worktree, so nothing was merged:\n")
                && error.contains("lint: 3 problems"),
            "{error}"
        );
        assert_eq!(status(root), "");
        assert_eq!(tgit(&blocked, &["log", "-1", "--format=%s"]), "other");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn target_checks_refuse_operations_in_progress() {
        let fx = Fixture::new();
        let root = fx.root.as_str();
        let wt = fx.worktree("feat");
        commit(&wt, "a.txt", "a\n", "add a");
        commit(&wt, "file.txt", "feature\n", "change file");
        // The preview's overlap lists (getMergePreview) are tested in workspace_git.rs. Its git.ts input: no operation in progress, so the preview has no inProgress.
        assert_eq!(operation_in_progress(root).await.unwrap(), None);

        // A merge in progress (a conflicted one, started outside Deckhand).
        let other = fx.worktree("other");
        commit(&other, "file.txt", "other side\n", "other change");
        commit(root, "file.txt", "main side\n", "main change");
        assert!(run_git(root, &["merge", "other"]).is_err());
        // The preview's inProgress ('merge') is operation_in_progress of the target.
        assert_eq!(operation_in_progress(root).await.unwrap(), Some(TargetOperation::Merge));
        let error = merge_worktree_into_current(&wt, root, WorktreeMergeMode::Merge, &MergeOptions::default())
            .await
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "A merge is in progress in the target: finish or abort it first"
        );
        tgit(root, &["merge", "--abort"]);
        // A rebase (its state directory) and a cherry-pick (its head file), as Git leaves them mid-way.
        let git_dir = Path::new(root).join(".git");
        std::fs::create_dir(git_dir.join("rebase-merge")).unwrap();
        let error = merge_worktree_into_current(&wt, root, WorktreeMergeMode::Squash, &MergeOptions::default())
            .await
            .unwrap_err();
        assert!(error.to_string().contains("A rebase is in progress"), "{error}");
        std::fs::remove_dir_all(git_dir.join("rebase-merge")).unwrap();
        let head = tgit(root, &["rev-parse", "HEAD"]);
        std::fs::write(git_dir.join("CHERRY_PICK_HEAD"), format!("{head}\n")).unwrap();
        assert_eq!(
            operation_in_progress(root).await.unwrap(),
            Some(TargetOperation::CherryPick)
        );
        std::fs::remove_file(git_dir.join("CHERRY_PICK_HEAD")).unwrap();
        // In a linked worktree as the target, its own state counts (not the main checkout's).
        std::fs::create_dir(git_dir.join("worktrees").join("other").join("rebase-apply")).unwrap();
        assert_eq!(
            operation_in_progress(&other).await.unwrap(),
            Some(TargetOperation::Rebase)
        );
        assert_eq!(operation_in_progress(root).await.unwrap(), None);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn conflicts_kept_or_aborted() {
        let fx = Fixture::new();
        let root = fx.root.as_str();
        let wt = fx.worktree("conflicting");
        commit(&wt, "file.txt", "feature side\n", "feature");
        commit(&wt, "added.txt", "added\n", "add");
        commit(root, "file.txt", "main side\n", "main");
        let head = tgit(root, &["rev-parse", "HEAD"]);
        // Unrelated uncommitted work in the target before the merge: an unstaged edit and an untracked file.
        write(root, "other.txt", "edited before the merge\n");
        write(root, "mine.txt", "untracked\n");
        let unrelated = || {
            assert_eq!(read(root, "other.txt"), "edited before the merge\n");
            assert_eq!(read(root, "mine.txt"), "untracked\n");
        };
        let none = MergeOptions::default();

        // A normal merge: conflicted result, MERGE_HEAD, markers; abort is git merge --abort.
        let normal = merge_worktree_into_current(&wt, root, WorktreeMergeMode::Merge, &none)
            .await
            .unwrap();
        assert_eq!(normal.result.conflicted, Some(true));
        assert_eq!(normal.result.conflicts, Some(vec!["file.txt".to_owned()]));
        assert_eq!(normal.result.conflict_count, Some(1));
        assert_eq!(normal.index_clean, Some(true));
        let text = read(root, "file.txt");
        assert!(text.contains("<<<<<<<") && text.contains(">>>>>>>"), "{text}");
        assert_eq!(operation_in_progress(root).await.unwrap(), Some(TargetOperation::Merge));
        abort_merge(root, None).await.unwrap();
        assert_eq!(operation_in_progress(root).await.unwrap(), None);
        assert_eq!(read(root, "file.txt"), "main side\n");
        assert_eq!(tgit(root, &["diff", "--cached", "--name-only"]), "");
        unrelated();

        // A squash merge has no MERGE_HEAD: the conflicts are what is in progress. Only Deckhand's own (clean index, same HEAD) is undone.
        let squash = merge_worktree_into_current(&wt, root, WorktreeMergeMode::Squash, &none)
            .await
            .unwrap();
        assert_eq!(squash.result.conflicted, Some(true));
        assert_eq!(squash.result.conflicts, Some(vec!["file.txt".to_owned()]));
        assert_eq!(squash.target_head.as_deref(), Some(head.as_str()));
        assert_eq!(squash.index_clean, Some(true));
        assert_eq!(
            operation_in_progress(root).await.unwrap(),
            Some(TargetOperation::Conflicts)
        );
        let error = abort_merge(root, None).await.unwrap_err().to_string();
        assert!(error.contains("Cannot abort this squash merge safely"), "{error}");
        let wrong_head = ConflictedMerge {
            mode: WorktreeMergeMode::Squash,
            target_head: Some("f".repeat(40)),
            index_clean: Some(true),
        };
        let error = abort_merge(root, Some(&wrong_head)).await.unwrap_err().to_string();
        assert!(error.contains("Cannot abort"), "{error}");
        let dirty = ConflictedMerge {
            mode: WorktreeMergeMode::Squash,
            target_head: Some(head.clone()),
            index_clean: Some(false),
        };
        let error = abort_merge(root, Some(&dirty)).await.unwrap_err().to_string();
        assert!(error.contains("Cannot abort"), "{error}");
        let started = ConflictedMerge {
            mode: WorktreeMergeMode::Squash,
            target_head: squash.target_head.clone(),
            index_clean: squash.index_clean,
        };
        abort_merge(root, Some(&started)).await.unwrap();
        assert_eq!(operation_in_progress(root).await.unwrap(), None);
        assert_eq!(read(root, "file.txt"), "main side\n");
        assert!(!Path::new(root).join("added.txt").exists());
        assert!(!Path::new(root).join(".git").join("SQUASH_MSG").exists());
        assert_eq!(sorted_lines(&status(root)), [" M other.txt", "?? mine.txt"]);
        unrelated();

        // Kept: left as Git left it (conflict markers, the clean part staged) for the user to resolve.
        merge_worktree_into_current(&wt, root, WorktreeMergeMode::Squash, &none)
            .await
            .unwrap();
        assert!(read(root, "file.txt").contains("<<<<<<<"));
        let lines = status(root);
        assert!(lines.split('\n').any(|line| line == "UU file.txt"), "{lines}");
        assert!(lines.split('\n').any(|line| line == "A  added.txt"), "{lines}");
        unrelated();
        // Git itself refuses a squash merge while unrelated changes are staged, so a conflicted squash always started clean.
        tgit(root, &["reset", "--merge"]);
        tgit(root, &["add", "other.txt"]);
        let error = merge_worktree_into_current(&wt, root, WorktreeMergeMode::Squash, &none)
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("overwritten by merge"), "{error}");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn branch_creation_commit_and_own_commits() {
        let fx = Fixture::new();
        let root = fx.root.as_str();
        let fresh = fx.worktree("fresh");
        let start = branch_creation_commit(&fresh, "fresh").await;
        assert_eq!(start.as_deref(), Some(tgit(&fresh, &["rev-parse", "HEAD"]).as_str()));
        assert_eq!(branch_has_own_commits(&fresh, "fresh").await, Some(false));
        // Fast-forwarded to a newer main: still nothing of its own.
        commit(root, "main.txt", "m\n", "main moves on");
        tgit(&fresh, &["merge", "--ff-only", "main"]);
        assert_eq!(branch_has_own_commits(&fresh, "fresh").await, Some(false));
        commit(&fresh, "f.txt", "f\n", "feature");
        assert_eq!(branch_has_own_commits(&fresh, "fresh").await, Some(true));
        assert_eq!(branch_creation_commit(&fresh, "fresh").await, start);
        assert_eq!(branch_creation_commit(&fresh, "").await, None);
        assert_eq!(branch_creation_commit(&fresh, "--all").await, None);
        assert_eq!(branch_creation_commit(&fresh, "missing").await, None);
        assert_eq!(branch_has_own_commits(&fresh, "missing").await, None);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn repo_context_worktrees_and_branches() {
        let fx = Fixture::new();
        let root = fx.root.as_str();
        assert_eq!(find_repo_root(Some(root)).await.unwrap(), root);
        assert_eq!(ensure_git_repo(Some(root)).await.unwrap(), root);
        let outside_dir = tempfile::tempdir().unwrap();
        let outside = real(outside_dir.path());
        assert_eq!(
            ensure_git_repo(Some(&outside)).await.unwrap_err().to_string(),
            "deckhand must be run from inside a git repository"
        );
        let wt = fx.worktree("feat");
        let context = resolve_repo_context(&wt).await.unwrap();
        assert_eq!(context.root, wt);
        assert_eq!(context.main_root.as_deref(), Some(root));
        assert_eq!(context.trust_root, root);
        assert_eq!(context.common_dir, format!("{root}/.git"));
        assert_eq!(find_git_common_dir(&wt).await.unwrap(), format!("{root}/.git"));
        let worktrees = list_worktrees(root).await.unwrap();
        assert_eq!(worktrees.len(), 2);
        assert!(worktrees[0].is_main && worktrees[0].branch == "main" && worktrees[0].path == root);
        assert!(!worktrees[1].is_main && worktrees[1].branch == "feat" && worktrees[1].path == wt);
        assert_eq!(list_local_branches(root, None).await.len(), 2);
        assert_eq!(
            resolve_default_branch(root, None, BranchWhere::Local).await.as_deref(),
            Some("main")
        );
        assert_eq!(
            resolve_default_branch(root, None, BranchWhere::Remote).await.as_deref(),
            Some("main")
        );
        let vars = worktree_template_vars("x", &wt).await.unwrap();
        assert_eq!(vars.repo_root, root);
        assert_eq!(vars.repo_parent, path::dirname(root));
        assert_eq!(vars.repo, path::basename(root));
        assert_eq!(worktree_branch_name(root, "feat/x", None).await.unwrap(), "feat/x");
        assert_eq!(
            worktree_branch_name(root, "x", Some("bad..{name}"))
                .await
                .unwrap_err()
                .to_string(),
            "Invalid branch name \"bad..x\" from worktree.branchName \"bad..{name}\""
        );
        assert_eq!(
            chosen_start(root, "-x").await.unwrap_err().to_string(),
            "Invalid base branch \"-x\""
        );
        assert_eq!(
            chosen_start(root, "nope").await.unwrap_err().to_string(),
            "Base branch nope does not exist here"
        );
        assert_eq!(chosen_start(root, "feat").await.unwrap().base_ref, "feat");
        let error = branch_start(root, root, Some(BranchFrom::Origin)).await.unwrap_err();
        assert_eq!(
            error.to_string(),
            "worktree.branchFrom is \"origin\", but the repository has no origin remote"
        );
        assert_eq!(branch_start(root, &wt, None).await.unwrap().base_ref, "feat");
        assert_eq!(
            branch_start(root, &wt, Some(BranchFrom::Default))
                .await
                .unwrap()
                .base_ref,
            "main"
        );
        assert_eq!(
            delete_local_branch(root, " main ").await.unwrap_err().to_string(),
            "refusing to delete protected branch main"
        );
        assert_eq!(
            delete_local_branch(root, "  ").await.unwrap_err().to_string(),
            "cannot delete an unnamed branch"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn creates_a_worktree_at_a_configured_location() {
        let fx = Fixture::new();
        let root = fx.root.as_str();
        let place_dir = tempfile::tempdir().unwrap();
        let place = real(place_dir.path());
        let settings = WorktreeSettings {
            location: Some(format!("{place}/{{repo}}-{{name}}")),
            ..WorktreeSettings::default()
        };
        let created = create_worktree_for_session("My Feature!", root, None, Some(&settings), None)
            .await
            .unwrap();
        let expected = format!("{place}/{}-my_feature", path::basename(root));
        assert_eq!(created.path, expected);
        assert_eq!(created.name, "my_feature");
        assert_eq!(created.branch, "my_feature");
        assert_eq!(created.origin, WorktreeOrigin::Created);
        assert_eq!(created.creator, WorktreeCreator::Fallback);
        assert_eq!(created.base_ref.as_deref(), Some("main"));
        assert_eq!(created.links, None);
        // Again: the existing worktree is reused.
        let again = create_worktree_for_session("My Feature!", root, None, Some(&settings), None)
            .await
            .unwrap();
        assert_eq!(again.origin, WorktreeOrigin::Existing);
        assert_eq!(again.base_ref, None);
        let hook = CreationHook {
            file: "/x/create-worktree.sh".into(),
            content: String::new(),
            fingerprint: String::new(),
        };
        let error = create_worktree_for_session("x", root, Some(&hook), None, Some("main"))
            .await
            .unwrap_err()
            .to_string();
        assert!(
            error.starts_with("This repository's worktree creation hook decides"),
            "{error}"
        );
        prune_worktrees(root).await.unwrap();
        // removeWorktree reads Deckhand's own worktree root: only with an isolated DECKHAND_HOME.
        if std::env::var_os("DECKHAND_HOME").is_some_and(|home| !home.is_empty()) {
            remove_worktree(&expected, root, Some("my_feature")).await.unwrap();
            assert!(!Path::new(&expected).exists());
            assert_eq!(list_worktrees(root).await.unwrap().len(), 1);
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn creation_hook_runs_the_reviewed_bytes() {
        let fx = Fixture::new();
        let root = fx.root.as_str();
        let target = fx.parent.path().join("hooked").to_string_lossy().into_owned();
        let content = format!(
            "read -r input\necho \"$input\" > \"{target}.stdin\"\necho \"dir=$CLAUDE_PROJECT_DIR\" > \"{target}.project\"\ngit worktree add -q -b hooked \"{target}\" >&2\necho noise\necho \"{target}\"\n"
        );
        let hook = CreationHook {
            file: "/reviewed/create-worktree.sh".into(),
            content,
            fingerprint: "f".into(),
        };
        let created = create_worktree_for_session("Hooked", root, Some(&hook), None, None)
            .await
            .unwrap();
        assert_eq!(created.creator, WorktreeCreator::Script);
        assert_eq!(created.origin, WorktreeOrigin::Created);
        assert_eq!(created.path, real(Path::new(&target)));
        assert_eq!(created.base_ref, None);
        assert_eq!(
            std::fs::read_to_string(format!("{target}.stdin")).unwrap(),
            format!("{{\"name\":\"hooked\",\"cwd\":\"{root}\"}}\n")
        );
        assert_eq!(
            std::fs::read_to_string(format!("{target}.project")).unwrap(),
            format!("dir={root}\n")
        );

        let failing = CreationHook {
            content: "echo oops >&2; exit 3".into(),
            ..hook.clone()
        };
        assert_eq!(
            run_create_script(&failing, "n", root, root)
                .await
                .unwrap_err()
                .to_string(),
            "create-worktree.sh failed with exit code 3: oops"
        );
        let silent = CreationHook {
            content: "true".into(),
            ..hook.clone()
        };
        assert_eq!(
            run_create_script(&silent, "n", root, root)
                .await
                .unwrap_err()
                .to_string(),
            "create-worktree.sh did not print a worktree path"
        );
        let relative = CreationHook {
            content: "echo rel/path".into(),
            ..hook
        };
        assert_eq!(
            run_create_script(&relative, "n", root, root)
                .await
                .unwrap_err()
                .to_string(),
            "create-worktree.sh returned a non-absolute path: rel/path"
        );
    }

    #[test]
    fn parses_the_porcelain_worktree_list() {
        let stdout = "worktree /repo.git\nbare\n\nworktree /a\nHEAD abc\nbranch refs/heads/feat/x\nlocked reason\n\nworktree /b\nHEAD def\ndetached\nprunable gitdir file points to non-existent location\n\nHEAD orphan\n";
        let entries = parse_worktree_list(stdout);
        assert_eq!(entries.len(), 3);
        assert!(entries[0].bare);
        assert_eq!(entries[1].branch, "feat/x");
        assert!(entries[1].locked && !entries[1].prunable);
        assert_eq!(entries[2].head, "def");
        assert!(entries[2].prunable && entries[2].branch.is_empty());
    }

    #[test]
    fn counts_like_parse_int() {
        assert_eq!(parse_int("12"), Some(12));
        assert_eq!(parse_int(" 7abc"), Some(7));
        assert_eq!(parse_int(""), None);
        assert_eq!(parse_int("x"), None);
    }

    #[test]
    fn own_commit_subjects() {
        for (subject, own) in [
            ("commit: x", true),
            ("commit (amend): x", true),
            ("commit (merge): x", true),
            ("cherry-pick: x", true),
            ("committed", false),
            ("merge main: Merge made by the 'ort' strategy.", true),
            ("merge main: Fast-forward", false),
            ("pull: Merge made by the 'ort' strategy.", true),
            ("reset: moving to HEAD~1", false),
            ("branch: Created from main", false),
        ] {
            assert_eq!(OWN_COMMIT.is_match(subject), own, "{subject}");
        }
    }

    #[test]
    fn sanitize_worktree_name_parity() {
        replay("git/sanitizeWorktreeName", |case: &Case| {
            // A lone surrogate arrives as U+FFFD, which the name rules replace just the same.
            Ok::<_, String>(sanitize_worktree_name(&case.arg_as::<String>(0)?))
        });
    }

    #[test]
    fn operation_problem_parity() {
        replay("git/operationProblem", |case: &Case| {
            Ok::<_, String>(target_problem(case.arg_as::<TargetOperation>(0)?))
        });
    }

    fn resolve_script_case(op: &DirOp) -> std::result::Result<Outcome, String> {
        if op.fn_name != "resolveCreateScript" {
            return Err(format!("unexpected fn {}", op.fn_name));
        }
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| error.to_string())?;
        let current = op.arg_str(0)?.to_owned();
        let main = if op.arg_is_undefined(1) {
            None
        } else {
            Some(op.arg_str(1)?.to_owned())
        };
        let found = runtime.block_on(resolve_create_script(&current, main.as_deref()));
        Ok(Outcome::ret(match found {
            Some(found) => serde_json::Value::String(found),
            None => tags::undefined(),
        }))
    }

    #[test]
    fn resolve_create_script_parity() {
        let cases = dir_cases("git");
        assert_eq!(cases.len(), 11);
        for case in cases {
            replay_dir(&case, |_root: &Path, op: &DirOp| {
                resolve_script_case(op).unwrap_or_else(Outcome::throws)
            });
        }
    }

    /// Committed fixture paths stay ASCII, so the TS-recorded case `resolveCreateScript("$ROOT/日本 😀")` ->
    /// `"$ROOT/日本 😀/.claude/scripts/create-worktree.sh"` is built at runtime here.
    #[test]
    fn resolve_create_script_non_ascii_dir() {
        let temp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(temp.path()).unwrap();
        let current = root.join("日本 😀");
        let script = current.join(CREATE_WORKTREE_SCRIPT);
        std::fs::create_dir_all(script.parent().unwrap()).unwrap();
        std::fs::write(&script, "#!/bin/bash\n").unwrap();
        let current = current.to_str().unwrap().to_owned();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let found = runtime.block_on(resolve_create_script(&current, None));
        assert_eq!(found, Some(format!("{current}/.claude/scripts/create-worktree.sh")));
        let missing = runtime.block_on(resolve_create_script(&format!("{current}/nope"), None));
        assert_eq!(missing, None);
    }
}
