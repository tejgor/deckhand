//! Port of worktreeLinks.ts (file I/O parts).
//!
//! Applying `worktree.symlink` and `worktree.files` to a newly created worktree, and `userSlug` (which reads the OS
//! user). Templates, validation and merging are pure and live in `deckhand_core::worktree_links`; LinkResult is
//! deckhand-protocol's [`WorktreeLinks`].

use std::path::Path;

use deckhand_core::error::Result;
use deckhand_protocol::data::{TemplateVars, WorktreeLinks, WorktreeSettings};

/// TS: worktreeLinks.ts userSlug (the OS user name as a branch-safe slug, `user` when nothing is left; see
/// `deckhand_core::worktree_links::slugify_user`).
pub fn user_slug() -> String {
    todo!("I6")
}

/// TS: worktreeLinks.ts inside (`target` is `root` or below it).
pub fn inside(root: &Path, target: &Path) -> bool {
    let _ = (root, target);
    todo!("I6")
}

/// TS: worktreeLinks.ts ensureParent (creates the parents of `relative` inside `root` one segment at a time, refusing
/// to pass through anything that resolves outside the worktree: `parent <dir> leaves the worktree`, `parent <dir> is
/// not a directory`). Returns the parent's real path.
pub async fn ensure_parent(root: &Path, relative: &str) -> Result<std::path::PathBuf> {
    let _ = (root, relative);
    todo!("I6")
}

/// TS: worktreeLinks.ts linkOne outcome.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkOutcome {
    Linked,
    /// Why the destination was skipped (`source missing, skipped`, `exists and is not a symlink, kept`, ...).
    Skipped(String),
}

/// TS: worktreeLinks.ts linkOne (existing real content is never replaced; an existing symlink is replaced
/// atomically).
pub async fn link_one(root: &Path, destination: &str, source: &Path) -> Result<LinkOutcome> {
    let _ = (root, destination, source);
    todo!("I6")
}

/// TS: worktreeLinks.ts applyWorktreeLinks `options`.
#[derive(Clone, Debug, PartialEq)]
pub struct LinkOptions {
    /// The checkout relative sources resolve against.
    pub launch_root: String,
    pub worktree_root: String,
    pub vars: TemplateVars,
}

/// TS: worktreeLinks.ts applyWorktreeLinks (never fails: every skipped or failed entry becomes a note).
pub async fn apply_worktree_links(settings: Option<&WorktreeSettings>, options: &LinkOptions) -> WorktreeLinks {
    let _ = (settings, options);
    todo!("I6")
}
