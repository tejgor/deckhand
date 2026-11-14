//! The work-items migration, run once over the state directory's notes (the rules are in
//! `deckhand_core::work_items_migration`; the design in `docs/future/work-items.md`, *Migration*).
//!
//! # A run
//!
//! 1. **Once.** `notes/work-items-migrated.json` (the marker) says it ran; then nothing happens. It lives in `notes/`
//!    because that is what it describes: a copy of `notes/` made before the migration (the backup, or a copy into the
//!    dev namespace to try it out) has no marker and migrates, a copy made after has one and doesn't. state.json is
//!    rewritten by every version, which keeps only the keys it knows, so a field there could be lost by an older one.
//! 2. **Backup first.** `notes/` (all of it but `trash/`) is copied to `notes/trash/work-items-migration-<time>/`, under
//!    a temporary name renamed when complete; a file `in-progress` inside marks it as the backup of a run that hasn't
//!    finished.
//! 3. **From the backup.** The inputs are read from the backup, not from `notes/`, so a run that was interrupted (its
//!    backup still `in-progress`) runs again on the same inputs. New IDs come from a generator seeded by the backup's
//!    name and the repository, so it gets the same IDs and writes the same files.
//! 4. **New files first**, each atomically: the lists (`notes/tasks/`) and the line notes (`notes/lines/`).
//! 5. **Then the old notes go**: each retired note file is removed from `notes/` when it is as the backup has it (the
//!    backup is where it now lives); one changed since the backup was taken moves into the backup beside its copy
//!    (`<stem>.changed.md`), and the report says so.
//! 6. **Then the marker**, the report (`migration-report.txt` in the backup), and the backup's `in-progress` goes.
//!
//! Which repository a note belongs to comes from the records: a session's repository is the main checkout of its
//! `repoRoot` (resolved with git; when that fails, `repoRoot` itself if a list exists under its hash); a worktree's is
//! its sessions', else the repository whose list links it (`wt=`). Notes whose repository or record can't be found
//! stay where they are (a session note is still read as one; the report lists the rest).

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use deckhand_core::error::{DhError, Result};
use deckhand_core::jsstr;
use deckhand_core::notes::repo_note_id;
use deckhand_core::task_list::TaskList;
use deckhand_core::work_items_migration::{
    MigrationCounts, NoteDir, RepoInput, SessionFacts, WorktreeFacts, migrate_repository, owning_session,
    worktree_facts,
};
use deckhand_protocol::Ordered;
use deckhand_protocol::data::SessionRecord;
use deckhand_protocol::records::WorktreeRecord;
use deckhand_sys::git::resolve_repo_context;
use deckhand_sys::notes_store::{TRASH_DIRECTORY, note_file_stem, note_stem};
use deckhand_sys::storage::write_atomic;
use sha2::{Digest, Sha256};

use crate::daemon::Daemon;

/// The marker under `notes/`.
pub const MARKER_FILE: &str = "work-items-migrated.json";
/// The backup's directory name under `notes/trash/`, before its time.
pub const BACKUP_PREFIX: &str = "work-items-migration-";
/// Inside a backup: the run it belongs to hasn't finished.
const IN_PROGRESS: &str = "in-progress";
/// Inside a backup: what the run did.
pub const REPORT_FILE: &str = "migration-report.txt";

const FILE_MODE: u32 = 0o600;
const DIR_MODE: u32 = 0o700;

/// What a call did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MigrationRun {
    /// False: the marker was there (or there was nothing to migrate and the marker was written).
    pub ran: bool,
    pub backup: Option<PathBuf>,
    pub counts: MigrationCounts,
    /// The report, a line each (also in the backup's `migration-report.txt`).
    pub report: Vec<String>,
}

impl MigrationRun {
    /// The line daemon.log gets.
    pub fn summary(&self) -> String {
        match &self.backup {
            Some(backup) if self.ran => format!(
                "work-items migration: {}; backup and report in {}",
                self.counts,
                backup.to_string_lossy()
            ),
            _ if self.ran => "work-items migration: nothing to migrate".to_owned(),
            _ => "work-items migration: done before".to_owned(),
        }
    }
}

/// Where a run stops early (tests: an interrupted run).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StopAt {
    Never,
    /// After writing the new files, before retiring the old ones.
    BeforeRetiring,
}

fn display(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn io_error(error: &std::io::Error, syscall: &str, path: &Path) -> DhError {
    DhError::io(error, syscall, &display(path))
}

async fn exists(path: &Path) -> bool {
    tokio::fs::metadata(path).await.is_ok()
}

async fn make_directory(directory: &Path) -> Result<()> {
    tokio::fs::DirBuilder::new()
        .recursive(true)
        .mode(DIR_MODE)
        .create(directory)
        .await
        .map_err(|error| io_error(&error, "mkdir", directory))
}

/// The entries of a directory, sorted by name (none when it doesn't exist).
async fn entries(directory: &Path) -> Result<Vec<(String, std::fs::FileType)>> {
    let mut reader = match tokio::fs::read_dir(directory).await {
        Ok(reader) => reader,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(io_error(&error, "scandir", directory)),
    };
    let mut out = Vec::new();
    while let Some(entry) = reader
        .next_entry()
        .await
        .map_err(|error| io_error(&error, "scandir", directory))?
    {
        let kind = entry
            .file_type()
            .await
            .map_err(|error| io_error(&error, "stat", &entry.path()))?;
        out.push((entry.file_name().to_string_lossy().into_owned(), kind));
    }
    out.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(out)
}

/// Copies the tree at `from` to `to` (directories 0700, files 0600), leaving out `skip` at its top level. Symbolic
/// links are not followed (Deckhand never makes any in `notes/`).
async fn copy_tree(from: &Path, to: &Path, skip: Option<&str>) -> Result<()> {
    make_directory(to).await?;
    let mut pending = vec![(from.to_path_buf(), to.to_path_buf(), skip)];
    while let Some((source, target, skip)) = pending.pop() {
        for (name, kind) in entries(&source).await? {
            if skip == Some(name.as_str()) {
                continue;
            }
            let (from, to) = (source.join(&name), target.join(&name));
            if kind.is_dir() {
                make_directory(&to).await?;
                pending.push((from, to, None));
            } else if kind.is_file() {
                tokio::fs::copy(&from, &to)
                    .await
                    .map_err(|error| DhError::io_dest(&error, "copyfile", &display(&from), &display(&to)))?;
                tokio::fs::set_permissions(&to, std::os::unix::fs::PermissionsExt::from_mode(FILE_MODE))
                    .await
                    .map_err(|error| io_error(&error, "chmod", &to))?;
            }
        }
    }
    Ok(())
}

/// `2026-11-14T12-00-00` (UTC, as trash file names have it).
fn stamp(now_ms: i64) -> String {
    jsstr::date::to_iso_string(now_ms)
        .unwrap_or_default()
        .chars()
        .take(19)
        .collect::<String>()
        .replace(':', "-")
}

/// The backup of an unfinished run, if there is one (the newest).
async fn unfinished_backup(trash: &Path) -> Result<Option<PathBuf>> {
    let mut found = None;
    for (name, kind) in entries(trash).await? {
        if kind.is_dir() && name.starts_with(BACKUP_PREFIX) && exists(&trash.join(&name).join(IN_PROGRESS)).await {
            found = Some(trash.join(name));
        }
    }
    Ok(found)
}

/// Makes the backup (see the module docs); a half-made one left by a crash is removed first.
async fn make_backup(notes_dir: &Path, now_ms: i64) -> Result<PathBuf> {
    let trash = notes_dir.join(TRASH_DIRECTORY);
    make_directory(&trash).await?;
    for (name, kind) in entries(&trash).await? {
        if kind.is_dir() && name.starts_with(&format!(".{BACKUP_PREFIX}")) {
            let stale = trash.join(name);
            tokio::fs::remove_dir_all(&stale)
                .await
                .map_err(|error| io_error(&error, "rm", &stale))?;
        }
    }
    let base = format!("{BACKUP_PREFIX}{}", stamp(now_ms));
    let mut name = base.clone();
    let mut n = 1;
    while exists(&trash.join(&name)).await {
        n += 1;
        name = format!("{base}-{n}");
    }
    let partial = trash.join(format!(".{name}.partial"));
    copy_tree(notes_dir, &partial, Some(TRASH_DIRECTORY)).await?;
    write_atomic(&partial.join(IN_PROGRESS), b"", FILE_MODE).await?;
    let backup = trash.join(&name);
    tokio::fs::rename(&partial, &backup)
        .await
        .map_err(|error| DhError::io_dest(&error, "rename", &display(&partial), &display(&backup)))?;
    Ok(backup)
}

/// The note files of a kind in `dir` (`<stem>.md`, not hidden), by stem.
async fn note_files(dir: &Path) -> Result<Vec<String>> {
    Ok(entries(dir)
        .await?
        .into_iter()
        .filter(|(_, kind)| kind.is_file())
        .filter_map(|(name, _)| note_stem(&name).map(str::to_owned))
        .collect())
}

/// A note's text; `None` (reported) when it isn't UTF-8.
async fn read_text(file: &Path, report: &mut Vec<String>) -> Result<Option<String>> {
    let bytes = match tokio::fs::read(file).await {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Some(String::new())),
        Err(error) => return Err(io_error(&error, "open", file)),
    };
    match String::from_utf8(bytes) {
        Ok(text) => Ok(Some(text)),
        Err(_) => {
            report.push(format!("{}: not UTF-8 text, left as it is", display(file)));
            Ok(None)
        }
    }
}

/// Eight hex digits from `seed`, `key` and a counter: the same run makes the same IDs.
fn seeded_ids(seed: String) -> impl FnMut() -> String {
    let mut n: u64 = 0;
    move || {
        n += 1;
        Sha256::digest(format!("{seed}\0{n}").as_bytes())
            .iter()
            .take(4)
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }
}

/// The list key (`notes/tasks/<key>.md`) of each session's repository, by `repoRoot`: the main checkout git names,
/// else `repoRoot` itself when a list exists under its hash. A repository it can't place is left out.
pub async fn repo_keys(sessions: &[Ordered<SessionRecord>], notes_dir: &Path) -> HashMap<String, String> {
    let mut keys = HashMap::new();
    for session in sessions {
        let root = session.repo_root.clone();
        if root.is_empty() || keys.contains_key(&root) {
            continue;
        }
        let mut key = None;
        if exists(Path::new(&root)).await
            && let Ok(context) = resolve_repo_context(&root).await
        {
            key = Some(repo_note_id(&context.trust_root));
        }
        if key.is_none() {
            let hashed = repo_note_id(&root);
            if exists(&notes_dir.join("tasks").join(format!("{hashed}.md"))).await {
                key = Some(hashed);
            }
        }
        if let Some(key) = key {
            keys.insert(root, key);
        }
    }
    keys
}

/// Runs the migration over `notes_dir` once (see the module docs). `repo_key` answers the list key of a session's
/// `repoRoot` (see [`repo_keys`]); `now_ms` names the backup.
pub async fn migrate_work_items(
    notes_dir: &Path,
    sessions: &[Ordered<SessionRecord>],
    worktrees: &[Ordered<WorktreeRecord>],
    repo_key: &dyn Fn(&str) -> Option<String>,
    now_ms: i64,
) -> Result<MigrationRun> {
    run(notes_dir, sessions, worktrees, repo_key, now_ms, StopAt::Never).await
}

pub(crate) async fn run(
    notes_dir: &Path,
    sessions: &[Ordered<SessionRecord>],
    worktrees: &[Ordered<WorktreeRecord>],
    repo_key: &dyn Fn(&str) -> Option<String>,
    now_ms: i64,
    stop: StopAt,
) -> Result<MigrationRun> {
    let marker = notes_dir.join(MARKER_FILE);
    if exists(&marker).await {
        return Ok(MigrationRun::default());
    }
    let trash = notes_dir.join(TRASH_DIRECTORY);
    let backup = match unfinished_backup(&trash).await? {
        Some(backup) => backup,
        None => {
            let mut any = false;
            for dir in [NoteDir::Worktrees, NoteDir::Sessions] {
                any |= !note_files(&notes_dir.join(dir.directory())).await?.is_empty();
            }
            any |= !note_files(&notes_dir.join("tasks")).await?.is_empty();
            if !any {
                make_directory(notes_dir).await?;
                write_marker(&marker, now_ms, None).await?;
                return Ok(MigrationRun {
                    ran: true,
                    ..MigrationRun::default()
                });
            }
            make_backup(notes_dir, now_ms).await?
        }
    };
    let seed = backup
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut report: Vec<String> = Vec::new();

    // ── What the records say ──
    let records: Vec<SessionRecord> = sessions.iter().map(|session| (**session).clone()).collect();
    let worktree_records: Vec<WorktreeRecord> = worktrees.iter().map(|record| (**record).clone()).collect();
    let session_facts: Vec<SessionFacts> = records.iter().map(SessionFacts::from_record).collect();
    let facts: Vec<WorktreeFacts> = worktree_facts(&worktree_records, &records);
    let key_of_session: HashMap<&str, String> = records
        .iter()
        .filter_map(|session| repo_key(&session.repo_root).map(|key| (session.id.as_str(), key)))
        .collect();

    // ── The inputs, from the backup ──
    let mut lists: BTreeMap<String, String> = BTreeMap::new();
    for stem in note_files(&backup.join("tasks")).await? {
        if let Some(text) = read_text(&backup.join("tasks").join(format!("{stem}.md")), &mut report).await? {
            lists.insert(stem, text);
        }
    }
    let mut line_notes = Vec::new();
    for stem in note_files(&backup.join(NoteDir::Lines.directory())).await? {
        let file = backup.join(NoteDir::Lines.directory()).join(format!("{stem}.md"));
        if let Some(text) = read_text(&file, &mut report).await? {
            line_notes.push((stem, text));
        }
    }
    // A worktree's repository: its owning session's (else any of its sessions'), else the list linking it.
    let key_of_worktree = |id: &str| -> Option<String> {
        if let Some(owner) = owning_session(id, &session_facts)
            && let Some(key) = key_of_session.get(owner.id.as_str())
        {
            return Some(key.clone());
        }
        lists
            .iter()
            .find(|(_, text)| {
                text.contains(&format!("wt={id}"))
                    && TaskList::parse(text)
                        .lines()
                        .iter()
                        .any(|line| line.meta.wt.as_deref() == Some(id))
            })
            .map(|(key, _)| key.clone())
    };
    let mut inputs: BTreeMap<String, RepoInput> = BTreeMap::new();
    for (key, text) in &lists {
        inputs.entry(key.clone()).or_default().list.clone_from(text);
    }
    let worktree_ids: HashMap<String, &str> = facts
        .iter()
        .map(|worktree| (note_file_stem(&worktree.id), worktree.id.as_str()))
        .collect();
    let worktree_dir = backup.join(NoteDir::Worktrees.directory());
    for stem in note_files(&worktree_dir).await? {
        let file = worktree_dir.join(format!("{stem}.md"));
        let Some(text) = read_text(&file, &mut report).await? else {
            continue;
        };
        let Some(id) = worktree_ids.get(&stem).copied() else {
            report.push(format!(
                "worktrees/{stem}.md: no worktree record or session names it; left as it is"
            ));
            continue;
        };
        let Some(key) = key_of_worktree(id) else {
            report.push(format!("worktrees/{stem}.md: its repository is unknown; left as it is"));
            continue;
        };
        inputs
            .entry(key)
            .or_default()
            .worktree_notes
            .push((id.to_owned(), text));
    }
    let session_ids: HashMap<String, &str> = records
        .iter()
        .map(|session| (note_file_stem(&session.id), session.id.as_str()))
        .collect();
    let session_dir = backup.join(NoteDir::Sessions.directory());
    let mut unplaced_sessions = 0;
    for stem in note_files(&session_dir).await? {
        let Some(id) = session_ids.get(&stem).copied() else {
            unplaced_sessions += 1;
            continue;
        };
        let Some(key) = key_of_session.get(id) else {
            unplaced_sessions += 1;
            continue;
        };
        let file = session_dir.join(format!("{stem}.md"));
        let Some(text) = read_text(&file, &mut report).await? else {
            continue;
        };
        inputs
            .entry(key.clone())
            .or_default()
            .session_notes
            .push((id.to_owned(), text));
    }
    if unplaced_sessions > 0 {
        report.push(format!(
            "{unplaced_sessions} session note(s) of sessions without a known repository stay session notes"
        ));
    }

    // ── Migrate each repository ──
    let mut counts = MigrationCounts::default();
    let mut writes: Vec<(PathBuf, String)> = Vec::new();
    let mut retired: Vec<(NoteDir, String)> = Vec::new();
    let mut repo_reports: Vec<String> = Vec::new();
    for (key, mut input) in inputs {
        input.line_notes.clone_from(&line_notes);
        input.sessions.clone_from(&session_facts);
        input.worktrees.clone_from(&facts);
        let mut new_id = seeded_ids(format!("{seed}\0{key}"));
        let output = migrate_repository(&input, now_ms, &mut new_id)?;
        if output.list != input.list {
            writes.push((notes_dir.join("tasks").join(format!("{key}.md")), output.list.clone()));
        }
        for file in &output.files {
            writes.push((
                notes_dir
                    .join(file.dir.directory())
                    .join(format!("{}.md", note_file_stem(&file.id))),
                file.text.clone(),
            ));
        }
        retired.extend(output.retired.iter().cloned());
        counts.add(&output.counts);
        repo_reports.push(format!("repository {key}:"));
        repo_reports.extend(output.report.iter().map(|line| format!("  {line}")));
    }
    report.splice(0..0, repo_reports);

    // ── New files, then the old ones go ──
    for (file, text) in &writes {
        if let Some(parent) = file.parent() {
            make_directory(parent).await?;
        }
        write_atomic(file, text.as_bytes(), FILE_MODE).await?;
    }
    if stop == StopAt::BeforeRetiring {
        return Err(DhError::msg(
            "work-items migration stopped before retiring the old notes",
        ));
    }
    for (dir, id) in &retired {
        let name = format!("{}.md", note_file_stem(id));
        let live = notes_dir.join(dir.directory()).join(&name);
        let Ok(now) = tokio::fs::read(&live).await else {
            // Retired by an earlier, interrupted run.
            continue;
        };
        let kept = tokio::fs::read(backup.join(dir.directory()).join(&name)).await.ok();
        if kept.as_deref() == Some(now.as_slice()) {
            tokio::fs::remove_file(&live)
                .await
                .map_err(|error| io_error(&error, "rm", &live))?;
        } else {
            let target = backup
                .join(dir.directory())
                .join(format!("{}.changed.md", note_file_stem(id)));
            make_directory(&backup.join(dir.directory())).await?;
            tokio::fs::rename(&live, &target)
                .await
                .map_err(|error| DhError::io_dest(&error, "rename", &display(&live), &display(&target)))?;
            report.push(format!(
                "{}/{name} changed after the backup was taken: what it says now is in the backup as {}",
                dir.directory(),
                display(&target)
            ));
        }
    }
    let relative = Path::new(TRASH_DIRECTORY).join(backup.file_name().unwrap_or_default());
    write_marker(&marker, now_ms, Some(&relative)).await?;
    let mut text = report.join("\n");
    text.push('\n');
    write_atomic(&backup.join(REPORT_FILE), text.as_bytes(), FILE_MODE).await?;
    let sentinel = backup.join(IN_PROGRESS);
    if let Err(error) = tokio::fs::remove_file(&sentinel).await
        && error.kind() != std::io::ErrorKind::NotFound
    {
        return Err(io_error(&error, "rm", &sentinel));
    }
    Ok(MigrationRun {
        ran: true,
        backup: Some(backup),
        counts,
        report,
    })
}

async fn write_marker(marker: &Path, now_ms: i64, backup: Option<&Path>) -> Result<()> {
    let mut value = serde_json::json!({
        "version": 1,
        "migratedAt": jsstr::date::to_iso_string(now_ms).unwrap_or_default(),
    });
    if let Some(backup) = backup {
        value["backup"] = serde_json::Value::String(display(backup));
    }
    let text = format!("{}\n", serde_json::to_string_pretty(&value).unwrap_or_default());
    write_atomic(marker, text.as_bytes(), FILE_MODE).await
}

impl Daemon {
    /// The work-items migration over this daemon's notes, once, logged to daemon.log (see the module docs). Stage 3
    /// calls it at start, with the stored records, before the notes are loaded.
    pub async fn migrate_work_items(
        &self,
        sessions: &[Ordered<SessionRecord>],
        worktrees: &[Ordered<WorktreeRecord>],
    ) -> Result<MigrationRun> {
        let notes_dir = self.paths.config_dir.join("notes");
        let keys = repo_keys(sessions, &notes_dir).await;
        let key = |root: &str| keys.get(root).cloned();
        let result = migrate_work_items(&notes_dir, sessions, worktrees, &key, jsstr::date::now_ms()).await;
        match &result {
            Ok(run) => {
                if run.ran {
                    self.log(&run.summary()).await;
                }
            }
            Err(error) => {
                self.log(&format!("work-items migration failed: {error}")).await;
            }
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::tests::{env_lock, local, test_daemon};
    use deckhand_core::notes::parse_checklist_line;
    use deckhand_core::tasks::{parse_note_task_link, without_task_meta};
    use serde_json::{Value, json};
    use std::os::unix::fs::PermissionsExt;

    const NOW: i64 = 1_794_657_600_000; // 2026-11-14T12:00:00Z
    const REPO: &str = "/work/app";

    fn temp() -> tempfile::TempDir {
        tempfile::Builder::new().prefix("deckhand-home-").tempdir().unwrap()
    }

    fn key() -> String {
        repo_note_id(REPO)
    }

    fn by_hash(root: &str) -> Option<String> {
        Some(repo_note_id(root))
    }

    fn session(id: &str, title: &str, worktree: Value) -> Ordered<SessionRecord> {
        let mut value = json!({
            "id": id, "title": title, "program": "claude", "command": "claude", "cwd": REPO, "repoRoot": REPO,
            "status": "exited", "createdAt": "2026-10-01T09:00:00.000Z", "updatedAt": "2026-10-01T09:00:00.000Z",
        });
        if !worktree.is_null() {
            value["worktree"] = worktree;
        }
        serde_json::from_value(value).unwrap()
    }

    fn records() -> (Vec<Ordered<SessionRecord>>, Vec<Ordered<WorktreeRecord>>) {
        let sessions = vec![
            session("s-main", "Main work", Value::Null),
            session(
                "s-feature",
                "Feature",
                json!({"mode": "managed", "origin": "created", "id": "w-feature", "branch": "feature"}),
            ),
        ];
        let worktrees = vec![
            serde_json::from_value(
                json!({"id": "w-feature", "path": "/work/feature", "createdAt": "2026-10-01T09:00:00.000Z"}),
            )
            .unwrap(),
        ];
        (sessions, worktrees)
    }

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    /// A notes directory as today's Deckhand leaves it.
    fn legacy_notes(home: &Path) -> PathBuf {
        let notes = home.join("notes");
        write(
            &notes.join("tasks").join(format!("{}.md", key())),
            "- [ ] Feature <!-- dh:t=a1 wt=w-feature -->\n  why we do it\n- [ ] Backlog item <!-- dh:t=a2 -->\n",
        );
        write(
            &notes.join("worktrees/w-feature.md"),
            "- [ ] write it\n- [x] plan it\nThinking.\n",
        );
        write(&notes.join("sessions/s-main.md"), "- [ ] tidy up\n");
        write(
            &notes.join("repos").join(format!("{}.md", key())),
            "Scratchpad stays.\n",
        );
        write(&notes.join("trash/2026-01-01T00-00-00_session_old_x.md"), "old trash\n");
        notes
    }

    /// Every file under `root` (relative path → contents), leaving out `skip` at the top.
    fn tree(root: &Path, skip: Option<&str>) -> BTreeMap<String, String> {
        let mut out = BTreeMap::new();
        let mut pending = vec![root.to_path_buf()];
        while let Some(dir) = pending.pop() {
            for entry in std::fs::read_dir(&dir).unwrap() {
                let path = entry.unwrap().path();
                let relative = path.strip_prefix(root).unwrap().to_string_lossy().into_owned();
                if skip.is_some_and(|skip| relative == skip) {
                    continue;
                }
                if path.is_dir() {
                    pending.push(path);
                } else {
                    out.insert(
                        relative,
                        String::from_utf8_lossy(&std::fs::read(&path).unwrap()).into_owned(),
                    );
                }
            }
        }
        out
    }

    fn mode(path: &Path) -> u32 {
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    fn migrate(notes: &Path, now: i64, stop: StopAt) -> Result<MigrationRun> {
        let (sessions, worktrees) = records();
        local(run(notes, &sessions, &worktrees, &by_hash, now, stop))
    }

    #[test]
    fn a_run_backs_up_writes_the_new_files_retires_the_old_and_leaves_its_marker() {
        let home = temp();
        let notes = legacy_notes(home.path());
        let before = tree(&notes, Some("trash"));
        let run = migrate(&notes, NOW, StopAt::Never).unwrap();
        assert!(run.ran);

        // The backup: everything but the trash, as it was, and the report.
        let backup = run.backup.clone().unwrap();
        assert_eq!(backup, notes.join("trash/work-items-migration-2026-11-14T12-00-00"));
        let mut kept = tree(&backup, None);
        let report = kept.remove(REPORT_FILE).unwrap();
        assert_eq!(kept, before);
        assert_eq!(report, format!("{}\n", run.report.join("\n")));
        assert!(report.starts_with(&format!("repository {}:\n", key())), "{report}");
        assert_eq!(mode(&backup), 0o700);
        assert_eq!(mode(&backup.join("worktrees/w-feature.md")), 0o600);

        // The new list and line notes; the worktree and session notes gone; the scratchpad and the trash untouched.
        let after = tree(&notes, Some("trash"));
        let list = &after[&format!("tasks/{}.md", key())];
        let titles: Vec<String> = TaskList::parse(list)
            .lines()
            .iter()
            .map(|line| format!("{}{} s={:?}", "  ".repeat(line.depth), line.title, line.meta.s))
            .collect();
        assert_eq!(
            titles,
            [
                "Feature s=Some(\"s-feature\")",
                "  write it s=None",
                "  plan it s=None",
                "Backlog item s=None",
                "Main work s=Some(\"s-main\")",
                "  tidy up s=None",
            ]
        );
        assert_eq!(
            after.get("lines/a1.md").map(String::as_str),
            Some("why we do it\n\nThinking.\n")
        );
        assert!(
            !after
                .keys()
                .any(|path| path.starts_with("worktrees/") || path.starts_with("sessions/"))
        );
        assert_eq!(after[&format!("repos/{}.md", key())], "Scratchpad stays.\n");
        assert!(notes.join("trash/2026-01-01T00-00-00_session_old_x.md").exists());
        assert_eq!(mode(&notes.join("lines")), 0o700);
        assert_eq!(mode(&notes.join("lines/a1.md")), 0o600);

        // The marker names the backup; the backup is no longer in progress.
        let marker: Value = serde_json::from_str(&after[MARKER_FILE]).unwrap();
        assert_eq!(
            marker,
            json!({"version": 1, "migratedAt": "2026-11-14T12:00:00.000Z", "backup": "trash/work-items-migration-2026-11-14T12-00-00"})
        );
        assert!(!backup.join(IN_PROGRESS).exists());
        assert_eq!(run.counts.hooked, 1);
        assert_eq!(run.counts.created, 1);
    }

    #[test]
    fn a_second_run_changes_nothing() {
        let home = temp();
        let notes = legacy_notes(home.path());
        migrate(&notes, NOW, StopAt::Never).unwrap();
        let first = tree(&notes, None);
        let again = migrate(&notes, NOW + 60_000, StopAt::Never).unwrap();
        assert!(!again.ran);
        assert_eq!(again.summary(), "work-items migration: done before");
        assert_eq!(tree(&notes, None), first);
    }

    #[test]
    fn an_interrupted_run_runs_again_to_the_same_result() {
        let (whole, interrupted) = (temp(), temp());
        let notes = legacy_notes(whole.path());
        migrate(&notes, NOW, StopAt::Never).unwrap();

        let broken = legacy_notes(interrupted.path());
        assert!(migrate(&broken, NOW, StopAt::BeforeRetiring).is_err());
        // The new files are there and so are the old ones; no marker.
        assert!(broken.join("lines/a1.md").exists());
        assert!(broken.join("worktrees/w-feature.md").exists());
        assert!(!broken.join(MARKER_FILE).exists());
        // Run again later: it finishes from the same backup with the same IDs.
        let run = migrate(&broken, NOW + 5_000, StopAt::Never).unwrap();
        assert!(run.ran);
        let without_marker = |notes: &Path| {
            let mut files = tree(notes, None);
            files.remove(MARKER_FILE);
            files
        };
        assert_eq!(without_marker(&broken), without_marker(&notes));
    }

    #[test]
    fn a_note_changed_after_the_backup_keeps_what_it_says_in_the_backup() {
        let home = temp();
        let notes = legacy_notes(home.path());
        assert!(migrate(&notes, NOW, StopAt::BeforeRetiring).is_err());
        write(
            &notes.join("sessions/s-main.md"),
            "- [ ] tidy up\n- [ ] written meanwhile\n",
        );
        let run = migrate(&notes, NOW, StopAt::Never).unwrap();
        let backup = run.backup.unwrap();
        assert!(!notes.join("sessions/s-main.md").exists());
        assert_eq!(
            std::fs::read_to_string(backup.join("sessions/s-main.changed.md")).unwrap(),
            "- [ ] tidy up\n- [ ] written meanwhile\n"
        );
        assert!(
            run.report
                .iter()
                .any(|line| line.contains("sessions/s-main.md changed after the backup"))
        );
    }

    #[test]
    fn with_nothing_to_migrate_it_only_leaves_its_marker() {
        let home = temp();
        let notes = home.path().join("notes");
        let run = migrate(&notes, NOW, StopAt::Never).unwrap();
        assert!(run.ran && run.backup.is_none());
        assert_eq!(tree(&notes, None).keys().collect::<Vec<_>>(), [MARKER_FILE]);
    }

    #[test]
    fn the_daemon_runs_it_once_and_logs_a_summary() {
        let home = temp();
        legacy_notes(home.path());
        let daemon = test_daemon(home.path());
        let (sessions, worktrees) = records();
        // Its repository isn't on this machine: the list under the hash of `repoRoot` is its list.
        let run = local(daemon.migrate_work_items(&sessions, &worktrees)).unwrap();
        assert!(run.ran);
        local(daemon.migrate_work_items(&sessions, &worktrees)).unwrap();
        let log = std::fs::read_to_string(home.path().join("daemon.log")).unwrap();
        let lines: Vec<&str> = log.lines().filter(|line| line.contains("work-items")).collect();
        assert_eq!(lines.len(), 1, "{log}");
        assert!(
            lines[0].contains("daemon work-items migration: 2 note(s) read, 1 line(s) put on a session"),
            "{}",
            lines[0]
        );
    }

    // ── The frozen legacy home (fixtures/state/home-v45) ──

    /// DECKHAND_HOME = `dir` while this lives.
    struct Home {
        _guard: std::sync::MutexGuard<'static, ()>,
        previous: Option<std::ffi::OsString>,
    }

    impl Home {
        fn set(dir: &Path) -> Home {
            let guard = env_lock();
            let previous = std::env::var_os(deckhand_sys::paths::HOME_ENV);
            // SAFETY: tests touching the environment hold server::tests::env_lock.
            unsafe { std::env::set_var(deckhand_sys::paths::HOME_ENV, dir) };
            Home {
                _guard: guard,
                previous,
            }
        }
    }

    impl Drop for Home {
        fn drop(&mut self) {
            // SAFETY: as in Home::set (the environment lock is still held).
            unsafe {
                match self.previous.take() {
                    Some(value) => std::env::set_var(deckhand_sys::paths::HOME_ENV, value),
                    None => std::env::remove_var(deckhand_sys::paths::HOME_ENV),
                }
            }
        }
    }

    /// Words of prose: bookkeeping comments, checkbox markers and `↗` lines aside.
    fn words(text: &str) -> Vec<String> {
        text.lines()
            .filter(|line| parse_note_task_link(line).is_none())
            .flat_map(|line| {
                let line = without_task_meta(line);
                let text = parse_checklist_line(&line).map_or(line, |item| item.text);
                jsstr::js_split_whitespace(&text)
                    .into_iter()
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    fn note_words(files: &BTreeMap<String, String>) -> HashMap<String, usize> {
        let mut out = HashMap::new();
        for (path, text) in files {
            let note = ["tasks/", "worktrees/", "sessions/", "lines/"]
                .iter()
                .any(|dir| path.starts_with(dir));
            if note && path.ends_with(".md") && !path.contains("/.") {
                for word in words(text) {
                    *out.entry(word).or_insert(0) += 1;
                }
            }
        }
        out
    }

    #[test]
    fn the_frozen_legacy_home_still_loads_and_migrates_without_losing_text() {
        let fixture = deckhand_testkit::repo_root().join("fixtures/state/home-v45");
        let home = temp();
        local(copy_tree(&fixture, home.path(), None)).unwrap();
        let notes = home.path().join("notes");
        let before = tree(&notes, Some("trash"));

        let state = {
            let _home = Home::set(home.path());
            local(deckhand_sys::storage::read_state()).unwrap().state
        };
        assert_eq!(state.sessions.len(), 17);
        // wt-0a was created by S04 (managed) and joined by its sub-session S05; wt-0c was only ever attached (S07).
        let facts: Vec<SessionFacts> = state
            .sessions
            .iter()
            .map(|session| SessionFacts::from_record(session))
            .collect();
        let owner = |wt: &str| owning_session(wt, &facts).map(|session| session.title.clone());
        assert_eq!(
            owner("7e000000-0000-4000-8000-00000000000a").as_deref(),
            Some("Feature A")
        );
        assert_eq!(
            owner("7e000000-0000-4000-8000-00000000000c").as_deref(),
            Some("Attached work")
        );

        let daemon = test_daemon(home.path());
        let run = local(daemon.migrate_work_items(&state.sessions, &state.worktrees)).unwrap();
        assert!(run.ran);
        let after = tree(&notes, Some("trash"));
        // Every word of the old lists and notes is in the new ones (or a note left where it was).
        let (old, new) = (note_words(&before), note_words(&after));
        for (word, times) in &old {
            let kept = new.get(word).copied().unwrap_or(0);
            assert!(
                kept >= *times,
                "{word:?}: {times} before, {kept} after\n{}",
                run.report.join("\n")
            );
        }
        // No worktree link is left in a list; the scratchpad is untouched; the fixture itself is never written.
        for (path, text) in &after {
            if path.starts_with("tasks/") {
                assert!(
                    TaskList::parse(text).lines().iter().all(|line| line.meta.wt.is_none()),
                    "{path}"
                );
            }
        }
        assert_eq!(
            after.get("repos/8528dc67ee723172.md"),
            before.get("repos/8528dc67ee723172.md")
        );
        assert!(
            fixture
                .join("notes/worktrees/7e000000-0000-4000-8000-00000000000c.md")
                .exists()
        );
        // A second run is a no-op.
        assert!(
            !local(daemon.migrate_work_items(&state.sessions, &state.worktrees))
                .unwrap()
                .ran
        );
    }
}
