//! Git I/O for the Git tab's Changes view, run by the daemon in a workspace root: one status read (porcelain v2 plus
//! numstat), one entry's bounded diff, and staging/unstaging of paths taken from the current status only. Never
//! commits, discards or touches the worktree's files. The view's model (grouping, keys, line counts) is
//! `deckhand_core::changes_model`; the records are deckhand-protocol's.

use std::collections::{HashMap, HashSet};
use std::os::unix::fs::MetadataExt;
use std::process::Stdio;
use std::time::Duration;

use indexmap::IndexMap;
use nix::sys::signal::Signal;
use tokio::io::AsyncReadExt;

use crate::git::{GitOptions, git};
use crate::proc::{self, ExecFailure, ExecOptions, GroupGuard};
use crate::workspace_git::parse_status;
use deckhand_core::changes_model::{
    LineCounts, Numstats, empty_changes, first_changed_line, group_changes, parse_numstat, untracked_diff,
};
use deckhand_core::error::{DhError, Result, errno_code};
use deckhand_core::jsstr::{self, path};
use deckhand_protocol::data::{ChangeDiff, ChangeEntry, ChangeGroup, ChangesRecord, JsNumber, StageMode};

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
    bytes[..bytes.len().min(BINARY_SNIFF_BYTES)].contains(&0)
}

/// TS: changesGit.ts countLines (a last line without a newline counts).
pub fn count_lines(bytes: &[u8]) -> i64 {
    let Some(&last) = bytes.last() else {
        return 0;
    };
    let lines = i64::try_from(bytes.iter().filter(|&&byte| byte == b'\n').count()).unwrap_or(i64::MAX);
    if last == b'\n' { lines } else { lines + 1 }
}

/// Node's `stat.mtimeMs` (fractional milliseconds).
#[allow(clippy::cast_precision_loss)]
fn mtime_ms(stat: &std::fs::Metadata) -> f64 {
    stat.mtime() as f64 * 1000.0 + stat.mtime_nsec() as f64 / 1e6
}

/// TS: changesGit.ts untrackedCounts (regular files only, never following symlinks; `cache` is updated and pruned).
/// Directories (nested repositories) and big files stay uncounted.
pub async fn untracked_counts(cwd: &str, files: &[String], cache: &mut UntrackedCounts) -> HashMap<String, LineCounts> {
    let mut counts = HashMap::new();
    let mut seen: HashSet<&str> = HashSet::new();
    for file in files.iter().take(MAX_UNTRACKED_COUNTED) {
        seen.insert(file.as_str());
        if file.ends_with('/') {
            continue;
        }
        let full = path::join(&[cwd, file]);
        let Ok(stat) = tokio::fs::symlink_metadata(&full).await else {
            continue;
        };
        if !stat.is_file() || stat.len() > MAX_UNTRACKED_COUNT_BYTES {
            continue;
        }
        let mtime = mtime_ms(&stat);
        if let Some(cached) = cache.get(file)
            && cached.size == stat.len()
            && cached.mtime_ms == mtime
        {
            counts.insert(file.clone(), cached.counts);
            continue;
        }
        let Ok(buffer) = tokio::fs::read(&full).await else {
            continue;
        };
        #[allow(clippy::cast_precision_loss)]
        let result = if is_binary(&buffer) {
            LineCounts {
                additions: None,
                deletions: None,
                binary: Some(true),
            }
        } else {
            LineCounts {
                additions: Some(JsNumber(count_lines(&buffer) as f64)),
                deletions: Some(JsNumber(0.0)),
                binary: None,
            }
        };
        cache.insert(
            file.clone(),
            UntrackedCount {
                size: stat.len(),
                mtime_ms: mtime,
                counts: result,
            },
        );
        counts.insert(file.clone(), result);
    }
    cache.retain(|file, _| seen.contains(file.as_str()));
    counts
}

/// TS: changesGit.ts readChanges (`Worktree directory is missing`; Git errors when it is not a worktree).
///
/// `git status --porcelain=v2 -z` (no optional locks, so polling never takes the index lock) plus staged/unstaged
/// numstat.
pub async fn read_changes(cwd: &str, cache: &mut UntrackedCounts) -> Result<ChangesSnapshot> {
    if !tokio::fs::metadata(cwd).await.is_ok_and(|stat| stat.is_dir()) {
        return Err(DhError::msg("Worktree directory is missing"));
    }
    let options = GitOptions::default().with_max_buffer(32 * 1024 * 1024);
    let status_args = [
        "--no-optional-locks",
        "status",
        "--porcelain=v2",
        "-z",
        "--branch",
        "--untracked-files=all",
    ];
    let staged_args = [
        "--no-optional-locks",
        "diff",
        "--cached",
        "--numstat",
        "-z",
        "-M",
        "--no-ext-diff",
        "--",
    ];
    let unstaged_args = ["--no-optional-locks", "diff", "--numstat", "-z", "--no-ext-diff", "--"];
    let (raw_status, staged, unstaged) = futures::try_join!(
        git(cwd, &status_args, Some(&options)),
        git(cwd, &staged_args, Some(&options)),
        git(cwd, &unstaged_args, Some(&options)),
    )?;
    let status = parse_status(&raw_status);
    let untracked = untracked_counts(cwd, &status.untracked, cache).await;
    // groupChanges only looks counts up by path, so the map's order does not matter.
    let untracked: IndexMap<String, LineCounts> = untracked.into_iter().collect();
    let numstat = Numstats {
        staged: Some(parse_numstat(&staged)),
        unstaged: Some(parse_numstat(&unstaged)),
        untracked: Some(untracked),
    };
    let all = group_changes(&status.entries, &numstat, usize::MAX).entries;
    let grouped = group_changes(&status.entries, &numstat, deckhand_core::changes_model::MAX_CHANGES);
    let branch = status.branch.clone().or_else(|| {
        status
            .oid
            .as_ref()
            .filter(|oid| !oid.is_empty())
            .map(|oid| format!("(detached {})", jsstr::js_slice(oid, 0, Some(7))))
    });
    let record = ChangesRecord {
        loaded: true,
        entries: grouped.entries,
        counts: grouped.counts,
        omitted: grouped.omitted,
        branch,
        ..empty_changes()
    };
    Ok(ChangesSnapshot {
        record,
        all,
        has_head: status.oid.as_ref().is_some_and(|oid| !oid.is_empty()),
    })
}

/// TS: changesGit.ts boundedGit result.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BoundedOutput {
    pub text: String,
    /// The output was cut at `max_bytes` (and then at its last complete line).
    pub truncated: bool,
}

/// Node's spawn error (`spawn git ENOENT`, with its code).
fn spawn_error(error: &std::io::Error) -> DhError {
    match errno_code(error) {
        Some(code) => DhError::with_code(code, format!("spawn git {code}")),
        None => DhError::msg(error.to_string()),
    }
}

/// `${code}` of Node's close event: the exit code, or `null` after a signal.
fn exit_code_text(code: Option<i32>) -> String {
    code.map_or_else(|| "null".to_owned(), |code| code.to_string())
}

/// The last 2000 characters of stderr (TS keeps `stderr.slice(-2000)`).
fn stderr_tail(bytes: &[u8]) -> String {
    jsstr::js_slice(&String::from_utf8_lossy(bytes), -2000, None)
}

/// TS: changesGit.ts boundedGit (stdout bounded to `max_bytes`: the rest is dropped and git stopped; READ_TIMEOUT:
/// `git diff timed out`; a failure: stderr's last line or `git exited with <code>`).
pub async fn bounded_git(cwd: &str, args: &[&str], max_bytes: usize) -> Result<BoundedOutput> {
    let mut full: Vec<&str> = vec!["-C", cwd];
    full.extend_from_slice(args);
    let mut command = proc::command("git", &full, &ExecOptions::default());
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().map_err(|error| spawn_error(&error))?;
    // Dropped early (timeout, or the caller gives up): the group is killed.
    let mut guard = GroupGuard::new(child.id(), true);
    let mut stdout_pipe = child.stdout.take();
    let mut stderr_pipe = child.stderr.take();
    let mut out: Vec<u8> = Vec::new();
    let mut err: Vec<u8> = Vec::new();
    let mut truncated = false;
    let mut status = None;
    let mut out_buf = vec![0u8; 64 * 1024];
    let mut err_buf = vec![0u8; 16 * 1024];
    let deadline = tokio::time::sleep(READ_TIMEOUT);
    tokio::pin!(deadline);
    while status.is_none() || stdout_pipe.is_some() || stderr_pipe.is_some() {
        tokio::select! {
            read = read_some(&mut stdout_pipe, &mut out_buf), if stdout_pipe.is_some() => match read {
                Ok(0) | Err(_) => stdout_pipe = None,
                Ok(n) => {
                    if !truncated {
                        out.extend_from_slice(&out_buf[..n]);
                        if out.len() > max_bytes {
                            truncated = true;
                            guard.signal(Signal::SIGTERM);
                        }
                    }
                }
            },
            read = read_some(&mut stderr_pipe, &mut err_buf), if stderr_pipe.is_some() => match read {
                Ok(0) | Err(_) => stderr_pipe = None,
                Ok(n) => {
                    err.extend_from_slice(&err_buf[..n]);
                    // Bounded like TS's `.slice(-2000)` (characters are at most 4 bytes).
                    if err.len() > 16_000 {
                        err.drain(..err.len() - 8000);
                    }
                }
            },
            result = child.wait(), if status.is_none() => {
                status = Some(result);
            }
            () = &mut deadline => {
                // The guard sends SIGKILL to the group as it drops.
                return Err(DhError::msg("git diff timed out"));
            }
        }
    }
    guard.disarm();
    let code = match status {
        Some(Ok(status)) => status.code(),
        Some(Err(error)) => return Err(spawn_error(&error)),
        None => None,
    };
    if code != Some(0) && !truncated {
        let tail = stderr_tail(&err);
        let last = jsstr::js_trim(&tail).split('\n').next_back().unwrap_or("").to_owned();
        return Err(DhError::msg(if last.is_empty() {
            format!("git exited with {}", exit_code_text(code))
        } else {
            last
        }));
    }
    let kept = &out[..out.len().min(max_bytes)];
    let text = String::from_utf8_lossy(kept).into_owned();
    // A cut output ends at the last complete line.
    let text = if truncated {
        text[..text.rfind('\n').unwrap_or(0)].to_owned()
    } else {
        text
    };
    Ok(BoundedOutput { text, truncated })
}

async fn read_some<R: tokio::io::AsyncRead + Unpin>(pipe: &mut Option<R>, buf: &mut [u8]) -> std::io::Result<usize> {
    match pipe {
        Some(pipe) => pipe.read(buf).await,
        None => Ok(0),
    }
}

/// `/^Binary files .* differ$/m.test(text) && !/^@@/m.test(text)` (JS line terminators: `\n`, `\r`, U+2028, U+2029).
fn binary_diff(text: &str) -> bool {
    let lines = || text.split(['\n', '\r', '\u{2028}', '\u{2029}']);
    let binary = lines().any(|line| {
        line.strip_prefix("Binary files ")
            .is_some_and(|rest| rest.ends_with(" differ"))
    });
    binary && !lines().any(|line| line.starts_with("@@"))
}

/// TS: changesGit.ts readChangeDiff (staged: `git diff --cached -M`, both paths of a rename; unstaged and conflicted:
/// `git diff`; untracked: the file's contents as additions, a symlink's target, a directory nothing).
pub async fn read_change_diff(cwd: &str, entry: &ChangeEntry) -> Result<ChangeDiff> {
    let diff = |text: String, truncated: bool, binary: bool, first_line: i64| ChangeDiff {
        group: entry.group,
        path: entry.path.clone(),
        text,
        truncated,
        binary,
        first_line,
    };
    if entry.group == ChangeGroup::Untracked {
        let file = path::join(&[cwd, &entry.path]);
        let stat = tokio::fs::symlink_metadata(&file)
            .await
            .map_err(|error| DhError::io(&error, "lstat", &file))?;
        if stat.file_type().is_symlink() {
            let target = tokio::fs::read_link(&file)
                .await
                .map_err(|error| DhError::io(&error, "readlink", &file))?;
            let text = untracked_diff(&format!("{}\n", target.to_string_lossy()));
            return Ok(diff(format!("symbolic link\n{text}"), false, false, 1));
        }
        if !stat.is_file() {
            return Ok(diff(String::new(), false, false, 1));
        }
        let mut handle = tokio::fs::File::open(&file)
            .await
            .map_err(|error| DhError::io(&error, "open", &file))?;
        let wanted = usize::try_from(stat.len()).unwrap_or(usize::MAX).min(MAX_DIFF_BYTES);
        let mut buffer = vec![0u8; wanted];
        let mut read = 0;
        while read < wanted {
            match handle.read(&mut buffer[read..]).await {
                Ok(0) => break,
                Ok(n) => read += n,
                Err(error) => return Err(DhError::io(&error, "read", &file)),
            }
        }
        let content = &buffer[..read];
        if is_binary(content) {
            return Ok(diff(String::new(), false, true, 1));
        }
        let truncated = stat.len() > read as u64;
        let mut text = String::from_utf8_lossy(content).into_owned();
        if truncated {
            text.truncate(text.rfind('\n').unwrap_or(0));
        }
        return Ok(diff(untracked_diff(&text), truncated, false, 1));
    }
    let staged = entry.group == ChangeGroup::Staged;
    let mut args: Vec<&str> = vec!["--no-optional-locks", "--literal-pathspecs", "diff"];
    if staged {
        args.extend(["--cached", "-M"]);
    }
    args.extend(["--no-color", "--no-ext-diff", "--"]);
    match (&entry.orig_path, staged) {
        (Some(orig), true) => args.extend([orig.as_str(), entry.path.as_str()]),
        _ => args.push(&entry.path),
    }
    let output = bounded_git(cwd, &args, MAX_DIFF_BYTES).await?;
    let binary = binary_diff(&output.text);
    let first = first_changed_line(&output.text);
    Ok(diff(output.text, output.truncated, binary, first))
}

/// TS: changesGit.ts gitWithInput (stdin `input`, WRITE_TIMEOUT: `git <subcommand> timed out`; a failure: stderr's
/// last non-empty line or `git exited with <code>`).
pub async fn git_with_input(cwd: &str, args: &[&str], input: &str) -> Result<()> {
    let mut full: Vec<&str> = vec!["-C", cwd];
    full.extend_from_slice(args);
    let options = ExecOptions {
        kill_signal: Signal::SIGKILL,
        // stdout is ignored in TS; keep whatever git prints from failing the run.
        max_buffer: 64 * 1024 * 1024,
        ..ExecOptions::default()
    }
    .stdin(input.as_bytes().to_vec())
    .timeout(WRITE_TIMEOUT);
    let error = match proc::exec("git", &full, &options).await {
        Ok(_) => return Ok(()),
        Err(error) => error,
    };
    if error.timed_out {
        let subcommand = args
            .iter()
            .find(|arg| !arg.starts_with('-'))
            .copied()
            .unwrap_or("undefined");
        return Err(DhError::msg(format!("git {subcommand} timed out")));
    }
    let code = match &error.failure {
        ExecFailure::Spawn { .. } => return Err(error.into()),
        ExecFailure::Exit { code } => Some(*code),
        ExecFailure::Signal { .. } | ExecFailure::MaxBuffer { .. } => None,
    };
    let tail = stderr_tail(error.stderr.as_bytes());
    let last = jsstr::js_trim(&tail)
        .split('\n')
        .rfind(|line| !line.is_empty())
        .map(str::to_owned);
    Err(DhError::msg(
        last.unwrap_or_else(|| format!("git exited with {}", exit_code_text(code))),
    ))
}

/// Paths go through stdin, NUL-separated and literal (no globs or pathspec magic), however many there are.
const PATHSPEC_ARGS: [&str; 2] = ["--pathspec-from-file=-", "--pathspec-file-nul"];

/// TS: changesGit.ts stagePaths (`git --literal-pathspecs add -A --pathspec-from-file=- --pathspec-file-nul`).
pub async fn stage_paths(cwd: &str, paths: &[String]) -> Result<()> {
    let args = ["--literal-pathspecs", "add", "-A", PATHSPEC_ARGS[0], PATHSPEC_ARGS[1]];
    git_with_input(cwd, &args, &paths.join("\0")).await
}

/// TS: changesGit.ts unstagePaths (`git restore --staged`, or `git rm --cached` before the first commit).
pub async fn unstage_paths(cwd: &str, paths: &[String], has_head: bool) -> Result<()> {
    let input = paths.join("\0");
    if has_head {
        let args = [
            "--literal-pathspecs",
            "restore",
            "--staged",
            PATHSPEC_ARGS[0],
            PATHSPEC_ARGS[1],
        ];
        git_with_input(cwd, &args, &input).await
    } else {
        // Without a HEAD commit there is nothing to restore from: remove the paths from the index instead.
        let args = [
            "--literal-pathspecs",
            "rm",
            "--cached",
            "-r",
            "-q",
            "--ignore-unmatch",
            PATHSPEC_ARGS[0],
            PATHSPEC_ARGS[1],
        ];
        git_with_input(cwd, &args, &input).await
    }
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

/// An entry's paths for unstaging: both sides of a rename.
fn unstage_paths_of(entry: &ChangeEntry) -> Vec<String> {
    match &entry.orig_path {
        Some(orig) => vec![orig.clone(), entry.path.clone()],
        None => vec![entry.path.clone()],
    }
}

fn count(n: usize) -> i64 {
    i64::try_from(n).unwrap_or(i64::MAX)
}

/// TS: changesGit.ts applyStage (one entry, or everything without a target, validated against `snapshot`: `<path>
/// is not among the unstaged|staged changes; refresh and try again`; stage all leaves conflicts for one-by-one
/// staging).
///
/// Stage: `git add -A` of the path (a deletion too; a conflict is marked resolved); stage all: `git add -A`, except
/// that conflicted files are left for one-by-one staging. Unstage: `git restore --staged` (both paths of a rename),
/// `git rm --cached` before the first commit; unstage all: every staged path.
pub async fn apply_stage(
    cwd: &str,
    snapshot: &ChangesSnapshot,
    mode: StageMode,
    target: Option<&StageTarget>,
) -> Result<StageOutcome> {
    let stage = mode == StageMode::Stage;
    if let Some(target) = target {
        let entry = snapshot
            .all
            .iter()
            .find(|candidate| candidate.group == target.group && candidate.path == target.path);
        let Some(entry) = entry.filter(|entry| stage == (entry.group != ChangeGroup::Staged)) else {
            return Err(DhError::msg(format!(
                "{} is not among the {} changes; refresh and try again",
                target.path,
                if stage { "unstaged" } else { "staged" }
            )));
        };
        if stage {
            stage_paths(cwd, std::slice::from_ref(&entry.path)).await?;
        } else {
            unstage_paths(cwd, &unstage_paths_of(entry), snapshot.has_head).await?;
        }
        return Ok(StageOutcome {
            changed: 1,
            skipped_conflicts: 0,
        });
    }
    if !stage {
        let staged: Vec<&ChangeEntry> = snapshot
            .all
            .iter()
            .filter(|entry| entry.group == ChangeGroup::Staged)
            .collect();
        if !staged.is_empty() {
            let paths: Vec<String> = staged.iter().flat_map(|entry| unstage_paths_of(entry)).collect();
            unstage_paths(cwd, &paths, snapshot.has_head).await?;
        }
        return Ok(StageOutcome {
            changed: count(staged.len()),
            skipped_conflicts: 0,
        });
    }
    let conflicts = snapshot
        .all
        .iter()
        .filter(|entry| entry.group == ChangeGroup::Conflicts)
        .count();
    let pending: Vec<&ChangeEntry> = snapshot
        .all
        .iter()
        .filter(|entry| matches!(entry.group, ChangeGroup::Unstaged | ChangeGroup::Untracked))
        .collect();
    // `[...new Set(paths)]`: first occurrence order.
    let mut seen: HashSet<&str> = HashSet::new();
    let unique: Vec<String> = pending
        .iter()
        .filter(|entry| seen.insert(entry.path.as_str()))
        .map(|entry| entry.path.clone())
        .collect();
    if conflicts == 0 {
        if !pending.is_empty() {
            git_with_input(cwd, &["add", "-A"], "").await?;
        }
    } else if !pending.is_empty() {
        stage_paths(cwd, &unique).await?;
    }
    Ok(StageOutcome {
        changed: count(unique.len()),
        skipped_conflicts: count(conflicts),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace_git::test_repo::{repo, run_git, temp, tgit, write};
    use deckhand_core::changes_model::find_change;
    use std::path::Path;

    async fn snapshot(cwd: &str) -> ChangesSnapshot {
        read_changes(cwd, &mut UntrackedCounts::new()).await.unwrap()
    }

    async fn shape(cwd: &str) -> Vec<String> {
        snapshot(cwd)
            .await
            .record
            .entries
            .iter()
            .map(|entry| {
                let from = entry
                    .orig_path
                    .as_ref()
                    .map(|orig| format!("{orig} → "))
                    .unwrap_or_default();
                format!("{}:{}:{from}{}", entry.group.as_str(), entry.status, entry.path)
            })
            .collect()
    }

    fn target(group: ChangeGroup, path: &str) -> StageTarget {
        StageTarget {
            group,
            path: path.to_owned(),
        }
    }

    fn found<'a>(entries: &'a [ChangeEntry], group: ChangeGroup, path: &str) -> &'a ChangeEntry {
        find_change(entries, group, path).unwrap_or_else(|| panic!("no {group} {path}"))
    }

    #[test]
    fn line_counting() {
        assert_eq!(count_lines(b""), 0);
        assert_eq!(count_lines(b"a"), 1);
        assert_eq!(count_lines(b"a\n"), 1);
        assert_eq!(count_lines(b"a\nb"), 2);
        assert!(is_binary(&[1, 0, 2]));
        let mut late = vec![b'a'; BINARY_SNIFF_BYTES];
        late.push(0);
        assert!(!is_binary(&late));
        assert!(binary_diff("diff --git a/x b/x\nBinary files a/x and b/x differ\n"));
        assert!(!binary_diff("Binary files a/x and b/x differ\n@@ -1 +1 @@\n"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn changes_read_diff_and_stage() {
        let (_dir, cwd) = repo();
        write(&cwd, "move me.txt", "a\nb\n");
        write(&cwd, "gone.txt", "x\n");
        tgit(&cwd, &["add", "."]);
        tgit(&cwd, &["commit", "-m", "more"]);
        write(&cwd, "file.txt", "first\nsecond\n");
        tgit(&cwd, &["add", "file.txt"]);
        write(&cwd, "file.txt", "first\nsecond\nthird\n");
        tgit(&cwd, &["mv", "move me.txt", "moved me.txt"]);
        std::fs::remove_file(Path::new(&cwd).join("gone.txt")).unwrap();
        write(&cwd, "new *.txt", "one\ntwo\nthree");
        write(&cwd, "new a.txt", "glob victim\n");
        write(&cwd, "blob.bin", [0u8, 1, 2, 3]);
        let mut snap = snapshot(&cwd).await;
        assert_eq!(snap.record.branch.as_deref(), Some("main"));
        assert!(snap.has_head);
        assert_eq!(
            shape(&cwd).await,
            vec![
                "staged:M:file.txt",
                "staged:R:move me.txt → moved me.txt",
                "unstaged:M:file.txt",
                "unstaged:D:gone.txt",
                "untracked:?:blob.bin",
                "untracked:?:new *.txt",
                "untracked:?:new a.txt",
            ]
        );
        let record = &snap.record;
        assert_eq!(
            [
                found(&record.entries, ChangeGroup::Staged, "file.txt").additions,
                found(&record.entries, ChangeGroup::Unstaged, "file.txt").additions,
                found(&record.entries, ChangeGroup::Unstaged, "gone.txt").deletions,
            ],
            [Some(1), Some(1), Some(1)]
        );
        assert_eq!(
            found(&record.entries, ChangeGroup::Untracked, "new *.txt").additions,
            Some(3)
        );
        assert_eq!(
            found(&record.entries, ChangeGroup::Untracked, "blob.bin").binary,
            Some(true)
        );

        // Diffs: staged vs unstaged sides, an untracked file as additions, binary detection.
        let staged = read_change_diff(&cwd, found(&snap.all, ChangeGroup::Staged, "file.txt"))
            .await
            .unwrap();
        assert!(staged.text.split('\n').any(|line| line == "+second"));
        assert!(!staged.text.contains("third"));
        assert_eq!(staged.first_line, 2);
        let unstaged = read_change_diff(&cwd, found(&snap.all, ChangeGroup::Unstaged, "file.txt"))
            .await
            .unwrap();
        assert!(unstaged.text.split('\n').any(|line| line == "+third"));
        assert_eq!(unstaged.first_line, 3);
        let rename = read_change_diff(&cwd, found(&snap.all, ChangeGroup::Staged, "moved me.txt"))
            .await
            .unwrap();
        assert!(rename.text.contains("rename from move me.txt"));
        let untracked = read_change_diff(&cwd, found(&snap.all, ChangeGroup::Untracked, "new *.txt"))
            .await
            .unwrap();
        assert_eq!(untracked.text, "@@ -0,0 +1,3 @@\n+one\n+two\n+three");
        assert_eq!(untracked.first_line, 1);
        let blob = read_change_diff(&cwd, found(&snap.all, ChangeGroup::Untracked, "blob.bin"))
            .await
            .unwrap();
        assert!(blob.binary);

        // Staging a glob-like name stages only that file; staging a deletion works; unstaging a rename restores both
        // paths.
        apply_stage(
            &cwd,
            &snap,
            StageMode::Stage,
            Some(&target(ChangeGroup::Untracked, "new *.txt")),
        )
        .await
        .unwrap();
        snap = snapshot(&cwd).await;
        assert!(find_change(&snap.all, ChangeGroup::Staged, "new *.txt").is_some());
        assert!(find_change(&snap.all, ChangeGroup::Untracked, "new a.txt").is_some());
        apply_stage(
            &cwd,
            &snap,
            StageMode::Stage,
            Some(&target(ChangeGroup::Unstaged, "gone.txt")),
        )
        .await
        .unwrap();
        snap = snapshot(&cwd).await;
        assert_eq!(found(&snap.all, ChangeGroup::Staged, "gone.txt").status, "D");
        apply_stage(
            &cwd,
            &snap,
            StageMode::Unstage,
            Some(&target(ChangeGroup::Staged, "moved me.txt")),
        )
        .await
        .unwrap();
        let moves: Vec<String> = shape(&cwd)
            .await
            .into_iter()
            .filter(|line| line.contains("move"))
            .collect();
        assert_eq!(moves, vec!["unstaged:D:move me.txt", "untracked:?:moved me.txt"]);
        // Paths not in the current status (or on the wrong side) are refused.
        snap = snapshot(&cwd).await;
        let refused = [
            (
                StageMode::Stage,
                ChangeGroup::Unstaged,
                "../outside",
                "not among the unstaged changes",
            ),
            (
                StageMode::Unstage,
                ChangeGroup::Untracked,
                "new a.txt",
                "not among the staged changes",
            ),
            (
                StageMode::Stage,
                ChangeGroup::Staged,
                "file.txt",
                "not among the unstaged changes",
            ),
        ];
        for (mode, group, path, message) in refused {
            let error = apply_stage(&cwd, &snap, mode, Some(&target(group, path)))
                .await
                .unwrap_err();
            assert!(error.to_string().contains(message), "{error}");
        }
        let error = apply_stage(&cwd, &snap, StageMode::Unstage, Some(&target(ChangeGroup::Staged, "x")))
            .await
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "x is not among the staged changes; refresh and try again"
        );

        // Stage all, then unstage all.
        let outcome = apply_stage(&cwd, &snap, StageMode::Stage, None).await.unwrap();
        assert_eq!(outcome.skipped_conflicts, 0);
        assert!(shape(&cwd).await.iter().all(|line| line.starts_with("staged:")));
        apply_stage(&cwd, &snapshot(&cwd).await, StageMode::Unstage, None)
            .await
            .unwrap();
        assert!(shape(&cwd).await.iter().all(|line| !line.starts_with("staged:")));
        let missing = read_changes(&format!("{cwd}/missing"), &mut UntrackedCounts::new()).await;
        assert_eq!(missing.unwrap_err().to_string(), "Worktree directory is missing");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn unborn_unstaging_and_conflicts() {
        let (_dir, cwd) = temp("deckhand-unborn-");
        tgit(&cwd, &["init", "-b", "main"]);
        std::fs::create_dir(Path::new(&cwd).join("dir")).unwrap();
        write(&cwd, "dir/a b.txt", "x\n");
        write(&cwd, "c.txt", "y\n");
        tgit(&cwd, &["add", "-A"]);
        let snap = snapshot(&cwd).await;
        assert!(!snap.has_head);
        assert_eq!(shape(&cwd).await, vec!["staged:A:c.txt", "staged:A:dir/a b.txt"]);
        apply_stage(
            &cwd,
            &snap,
            StageMode::Unstage,
            Some(&target(ChangeGroup::Staged, "dir/a b.txt")),
        )
        .await
        .unwrap();
        assert_eq!(shape(&cwd).await, vec!["staged:A:c.txt", "untracked:?:dir/a b.txt"]);
        apply_stage(&cwd, &snapshot(&cwd).await, StageMode::Unstage, None)
            .await
            .unwrap();
        assert_eq!(shape(&cwd).await, vec!["untracked:?:c.txt", "untracked:?:dir/a b.txt"]);

        // A conflict: stage all stages the rest and reports the conflict; staging the file itself marks it resolved.
        let (_conflicted_dir, conflicted) = repo();
        tgit(&conflicted, &["checkout", "-b", "other"]);
        write(&conflicted, "file.txt", "other\n");
        tgit(&conflicted, &["commit", "-am", "other"]);
        tgit(&conflicted, &["checkout", "main"]);
        write(&conflicted, "file.txt", "main\n");
        tgit(&conflicted, &["commit", "-am", "main"]);
        let _ = run_git(&conflicted, &["merge", "other"]);
        write(&conflicted, "extra.txt", "extra\n");
        let snap = snapshot(&conflicted).await;
        assert_eq!(
            shape(&conflicted).await,
            vec!["conflicts:U:file.txt", "untracked:?:extra.txt"]
        );
        let diff = read_change_diff(&conflicted, found(&snap.all, ChangeGroup::Conflicts, "file.txt"))
            .await
            .unwrap();
        assert!(diff.text.split('\n').any(|line| line.starts_with("diff --cc file.txt")));
        assert!(diff.first_line >= 1);
        assert_eq!(
            apply_stage(&conflicted, &snap, StageMode::Stage, None).await.unwrap(),
            StageOutcome {
                changed: 1,
                skipped_conflicts: 1
            }
        );
        assert_eq!(
            shape(&conflicted).await,
            vec!["conflicts:U:file.txt", "staged:A:extra.txt"]
        );
        let snap = snapshot(&conflicted).await;
        apply_stage(
            &conflicted,
            &snap,
            StageMode::Stage,
            Some(&target(ChangeGroup::Conflicts, "file.txt")),
        )
        .await
        .unwrap();
        assert_eq!(
            shape(&conflicted).await,
            vec!["staged:A:extra.txt", "staged:M:file.txt"]
        );
        // Unstaging/staging never ends the merge.
        let merge_head = Path::new(&conflicted).join(".git").join("MERGE_HEAD");
        assert!(merge_head.exists());
        apply_stage(&conflicted, &snapshot(&conflicted).await, StageMode::Unstage, None)
            .await
            .unwrap();
        assert!(merge_head.exists());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn untracked_counts_cache_and_bounded_diffs() {
        let (_dir, cwd) = repo();
        write(&cwd, "a.txt", "1\n2\n");
        write(&cwd, "big.txt", "line\n".repeat(60_000));
        std::os::unix::fs::symlink("file.txt", Path::new(&cwd).join("link")).unwrap();
        let mut cache = UntrackedCounts::new();
        cache.insert(
            "stale".to_owned(),
            UntrackedCount {
                size: 1,
                mtime_ms: 0.0,
                counts: LineCounts::default(),
            },
        );
        let files = vec!["a.txt".to_owned(), "link".to_owned(), "dir/".to_owned()];
        let counts = untracked_counts(&cwd, &files, &mut cache).await;
        assert_eq!(counts.len(), 1);
        assert_eq!(counts["a.txt"].additions, Some(JsNumber(2.0)));
        assert_eq!(cache.keys().collect::<Vec<_>>(), vec!["a.txt"]);
        // An untracked symlink shows its target.
        let link = ChangeEntry {
            group: ChangeGroup::Untracked,
            path: "link".to_owned(),
            orig_path: None,
            status: "?".to_owned(),
            conflict: None,
            additions: None,
            deletions: None,
            binary: None,
        };
        let diff = read_change_diff(&cwd, &link).await.unwrap();
        assert_eq!(diff.text, "symbolic link\n@@ -0,0 +1,1 @@\n+file.txt");
        // A big untracked file is cut at the last complete line.
        let big = ChangeEntry {
            path: "big.txt".to_owned(),
            ..link.clone()
        };
        let diff = read_change_diff(&cwd, &big).await.unwrap();
        assert!(diff.truncated);
        assert!(diff.text.ends_with("+line"));
        // A big diff is bounded too.
        tgit(&cwd, &["add", "big.txt"]);
        let staged = ChangeEntry {
            group: ChangeGroup::Staged,
            status: "A".to_owned(),
            ..big
        };
        let diff = read_change_diff(&cwd, &staged).await.unwrap();
        assert!(diff.truncated);
        assert!(diff.text.len() <= MAX_DIFF_BYTES);
        assert!(diff.text.ends_with("+line"));
        let failed = bounded_git(&cwd, &["diff", "--no-such-option"], 100).await.unwrap_err();
        assert!(!failed.to_string().is_empty());
        // Git's usage text: the error is its last non-empty line.
        let error = git_with_input(&cwd, &["add", "--no-such-option"], "")
            .await
            .unwrap_err();
        assert!(
            !error.to_string().contains('\n') && !error.to_string().is_empty(),
            "{error}"
        );
        let error = git_with_input(&cwd, &["-c", "x", "nope-command"], "")
            .await
            .unwrap_err();
        assert!(!error.to_string().is_empty());
    }
}
