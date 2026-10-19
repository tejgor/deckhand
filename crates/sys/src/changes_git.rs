//! Port of changesGit.ts.
//!
//! Git I/O for the Git tab's Changes view, run by the daemon in a workspace root: one status read (porcelain v2 plus
//! numstat), one entry's bounded diff, and staging/unstaging of paths taken from the current status only. Never
//! commits, discards or touches the worktree's files. The view's model (grouping, keys, line counts) is
//! `deckhand_core::changes_model`; the records are deckhand-protocol's.

use std::collections::HashMap;
use std::time::Duration;

use deckhand_core::changes_model::LineCounts;
use deckhand_core::error::Result;
use deckhand_protocol::data::{ChangeDiff, ChangeEntry, ChangeGroup, ChangesRecord, StageMode};

/// TS: changesGit.ts MAX_DIFF_BYTES (one entry's diff at most).
pub const MAX_DIFF_BYTES: usize = 256 * 1024;
/// TS: changesGit.ts MAX_UNTRACKED_COUNTED (untracked files whose lines are counted).
pub const MAX_UNTRACKED_COUNTED: usize = 500;
/// TS: changesGit.ts MAX_UNTRACKED_COUNT_BYTES (bigger untracked files stay uncounted).
pub const MAX_UNTRACKED_COUNT_BYTES: u64 = 1024 * 1024;
/// TS: changesGit.ts BINARY_SNIFF_BYTES (a NUL in the first this many bytes means binary).
pub const BINARY_SNIFF_BYTES: usize = 8000;
/// TS: changesGit.ts READ_TIMEOUT_MS.
pub const READ_TIMEOUT: Duration = Duration::from_millis(8000);
/// TS: changesGit.ts WRITE_TIMEOUT_MS.
pub const WRITE_TIMEOUT: Duration = Duration::from_millis(60_000);

/// TS: changesGit.ts ChangesSnapshot: the record the view shows (capped) plus every entry (for validation).
#[derive(Clone, Debug, PartialEq)]
pub struct ChangesSnapshot {
    pub record: ChangesRecord,
    pub all: Vec<ChangeEntry>,
    pub has_head: bool,
}

/// One cached untracked line count (TS `{size, mtimeMs, counts}`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UntrackedCount {
    pub size: u64,
    pub mtime_ms: f64,
    pub counts: LineCounts,
}

/// TS: changesGit.ts UntrackedCounts (by path, reused while size and mtime are unchanged).
pub type UntrackedCounts = HashMap<String, UntrackedCount>;

/// TS: changesGit.ts isBinary.
pub fn is_binary(bytes: &[u8]) -> bool {
    let _ = bytes;
    todo!("I4")
}

/// TS: changesGit.ts countLines (a last line without a newline counts).
pub fn count_lines(bytes: &[u8]) -> i64 {
    let _ = bytes;
    todo!("I4")
}

/// TS: changesGit.ts untrackedCounts (regular files only, never following symlinks; `cache` is updated and pruned).
pub async fn untracked_counts(cwd: &str, files: &[String], cache: &mut UntrackedCounts) -> HashMap<String, LineCounts> {
    let _ = (cwd, files, cache);
    todo!("I4")
}

/// TS: changesGit.ts readChanges (`Worktree directory is missing`; Git errors when it is not a worktree).
pub async fn read_changes(cwd: &str, cache: &mut UntrackedCounts) -> Result<ChangesSnapshot> {
    let _ = (cwd, cache);
    todo!("I4")
}

/// TS: changesGit.ts boundedGit result.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BoundedOutput {
    pub text: String,
    /// The output was cut at `max_bytes` (and then at its last complete line).
    pub truncated: bool,
}

/// TS: changesGit.ts boundedGit (stdout bounded to `max_bytes`: the rest is dropped and git stopped; READ_TIMEOUT:
/// `git diff timed out`; a failure: stderr's last line or `git exited with <code>`).
pub async fn bounded_git(cwd: &str, args: &[&str], max_bytes: usize) -> Result<BoundedOutput> {
    let _ = (cwd, args, max_bytes);
    todo!("I4")
}

/// TS: changesGit.ts readChangeDiff (staged: `git diff --cached -M`, both paths of a rename; unstaged and conflicted:
/// `git diff`; untracked: the file's contents as additions, a symlink's target, a directory nothing).
pub async fn read_change_diff(cwd: &str, entry: &ChangeEntry) -> Result<ChangeDiff> {
    let _ = (cwd, entry);
    todo!("I4")
}

/// TS: changesGit.ts gitWithInput (stdin `input`, WRITE_TIMEOUT: `git <subcommand> timed out`; a failure: stderr's
/// last non-empty line or `git exited with <code>`).
pub async fn git_with_input(cwd: &str, args: &[&str], input: &str) -> Result<()> {
    let _ = (cwd, args, input);
    todo!("I4")
}

/// TS: changesGit.ts stagePaths (`git --literal-pathspecs add -A --pathspec-from-file=- --pathspec-file-nul`).
pub async fn stage_paths(cwd: &str, paths: &[String]) -> Result<()> {
    let _ = (cwd, paths);
    todo!("I4")
}

/// TS: changesGit.ts unstagePaths (`git restore --staged`, or `git rm --cached` before the first commit).
pub async fn unstage_paths(cwd: &str, paths: &[String], has_head: bool) -> Result<()> {
    let _ = (cwd, paths, has_head);
    todo!("I4")
}

/// TS: changesGit.ts applyStage `target`: one entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StageTarget {
    pub group: ChangeGroup,
    pub path: String,
}

/// TS: changesGit.ts applyStage result.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StageOutcome {
    pub changed: i64,
    pub skipped_conflicts: i64,
}

/// TS: changesGit.ts applyStage (one entry, or everything without a target, validated against `snapshot`: `<path>
/// is not among the unstaged|staged changes; refresh and try again`; stage all leaves conflicts for one-by-one
/// staging).
pub async fn apply_stage(
    cwd: &str,
    snapshot: &ChangesSnapshot,
    mode: StageMode,
    target: Option<&StageTarget>,
) -> Result<StageOutcome> {
    let _ = (cwd, snapshot, mode, target);
    todo!("I4")
}
