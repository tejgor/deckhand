//! Port of worktreeLinks.ts (pure parts).
//!
//! Declarative worktree settings (the `worktree` section of global defaults or deckhand.json): where new worktrees go
//! and which heavy directories / private files are symlinked into them. Applying the links to a new worktree (and
//! `userSlug`, which reads the OS user) is I/O and lives in `deckhand_sys::worktree_links`.

use serde_json::Value;

use crate::error::Result;
use deckhand_protocol::data::{BranchFrom, TemplateVars, WorktreeSettings};

/// TS: worktreeLinks.ts BRANCH_FROM.
pub const BRANCH_FROM: [BranchFrom; 3] = [BranchFrom::Current, BranchFrom::Default, BranchFrom::Origin];

/// TS: worktreeLinks.ts PLACEHOLDERS (`{name}` and friends in worktree templates).
pub const PLACEHOLDERS: [&str; 5] = ["name", "repo", "repoParent", "repoRoot", "home"];

/// TS: worktreeLinks.ts MAX_ENTRIES (per `symlink` list and `files` map).
pub const MAX_ENTRIES: usize = 50;

/// TS: worktreeLinks.ts MAX_PATH (UTF-16 units).
pub const MAX_PATH: usize = 4096;

/// TS: worktreeLinks.ts expandWorktreeTemplate (placeholders and a leading `~/`; unknown placeholders and unbalanced
/// braces are errors; the result is not resolved).
pub fn expand_worktree_template(template: &str, vars: &TemplateVars) -> Result<String> {
    let _ = (template, vars);
    todo!("P2")
}

/// TS: worktreeLinks.ts worktreeLocation (the template must expand to an absolute path; resolved).
pub fn worktree_location(template: &str, vars: &TemplateVars) -> Result<String> {
    let _ = (template, vars);
    todo!("P2")
}

/// TS: worktreeLinks.ts BranchVars.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BranchVars<'a> {
    pub name: &'a str,
    pub user: &'a str,
}

/// TS: worktreeLinks.ts expandBranchName (`{name}` and `{user}` only).
pub fn expand_branch_name(template: &str, vars: &BranchVars<'_>) -> Result<String> {
    let _ = (template, vars);
    todo!("P2")
}

/// TS: worktreeLinks.ts branchNameProblem (a static `git check-ref-format --branch`; `None`: plausible).
pub fn branch_name_problem(name: &str) -> Option<&'static str> {
    let _ = name;
    todo!("P2")
}

/// The pure half of worktreeLinks.ts userSlug: an OS user name as a branch-safe slug (`user` when nothing is left).
pub fn slugify_user(name: &str) -> String {
    let _ = name;
    todo!("P2")
}

/// TS: worktreeLinks.ts DUMMY (the vars templates are checked with).
pub fn dummy_template_vars() -> TemplateVars {
    todo!("P2")
}

/// TS: worktreeLinks.ts templateString (a non-empty string of at most MAX_PATH without NUL that expands; errors
/// read `Invalid <label>` / `Invalid <label>: <why>`).
pub fn template_string(value: &Value, label: &str) -> Result<String> {
    let _ = (value, label);
    todo!("P2")
}

/// TS: worktreeLinks.ts validRelativePath (normalized, relative, inside its root, not `.git`). Non-strings are never
/// valid: callers pass `value.as_str()`.
pub fn valid_relative_path(value: &str) -> bool {
    let _ = value;
    todo!("P2")
}

/// TS: worktreeLinks.ts validateWorktreeSettings (the `worktree` value of a deckhand.json-shaped object).
pub fn validate_worktree_settings(value: &Value) -> Result<WorktreeSettings> {
    let _ = value;
    todo!("P2")
}

/// TS: worktreeLinks.ts mergeWorktreeSettings (repository scalars win; symlinks are a deduplicated union, global
/// first; files merge by destination, repository winning). The TS literal's key order is location, branchFrom,
/// branchName, hook, symlink, files: callers that serialise the result record that order.
pub fn merge_worktree_settings(
    global: Option<&WorktreeSettings>,
    repository: Option<&WorktreeSettings>,
) -> Option<WorktreeSettings> {
    let _ = (global, repository);
    todo!("P2")
}
