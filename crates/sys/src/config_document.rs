//! Port of projectConfigDocument.ts (file I/O parts).
//!
//! C's two editable documents: the user's global defaults (config.json `defaults`, pretty-printed; its revision hashes
//! that value) and the repository's deckhand.json in the main checkout (its revision hashes the file's bytes). Saves
//! are revision-checked, serialized per file, written atomically, and keep trust for edits made through Deckhand. The
//! pure parts (checkDraft, globalRevision, globalDocument, the starter text) are in
//! `deckhand_core::project_config_document`; the document types are deckhand-protocol's.

use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;

use deckhand_core::config_draft::MAX_CONFIG_BYTES;
use deckhand_core::error::{DhError, Result};
use deckhand_core::jsstr::path;
use deckhand_core::project_config::{PROJECT_CONFIG_FILE, parse_project_config, sha256, trust_project_config};
use deckhand_core::project_config_document::{STARTER_PROJECT_CONFIG, check_draft, global_document, global_revision};
use deckhand_protocol::data::{ConfigTargetKind, ConfigTargets, ProjectConfigDocument, SavedConfigDocument};

use crate::async_util::KeyedQueue;
use crate::config::{PreviousDocument, ReadBounded, read_bounded_utf8, saved_project_trust};
use crate::git::{RepoContext, resolve_repo_context};
use crate::paths;
use crate::storage::{load_app_config, update_app_config};

/// TS: projectConfigDocument.ts readConfigTargets (each target stays available when the other cannot be read; its
/// error message is reported instead).
pub async fn read_config_targets(cwd: &str) -> ConfigTargets {
    let mut targets = ConfigTargets::default();
    match read_global_defaults_document().await {
        Ok(document) => targets.global = Some(document),
        Err(error) => targets.global_error = Some(error.to_string()),
    }
    match read_project_config_document(cwd).await {
        Ok(document) => targets.repository = Some(document),
        Err(error) => targets.repository_error = Some(error.to_string()),
    }
    targets
}

/// TS: projectConfigDocument.ts repositoryContext (`Bare repositories have no main checkout, so no repository
/// deckhand.json; use global defaults`). The returned context always has `main_root`.
pub async fn repository_context(cwd: &str) -> Result<RepoContext> {
    let context = resolve_repo_context(cwd).await?;
    if context.main_root.is_none() {
        return Err(DhError::msg(
            "Bare repositories have no main checkout, so no repository deckhand.json; use global defaults",
        ));
    }
    Ok(context)
}

/// TS: projectConfigDocument.ts readDocumentAt result: the document and the file's mode (0644 for a new file).
#[derive(Clone, Debug, PartialEq)]
pub struct DocumentAt {
    pub document: ProjectConfigDocument,
    pub mode: u32,
}

/// TS: projectConfigDocument.ts readDocumentAt (malformed JSON is allowed so it can be repaired; missing: the
/// starter text with a `null` revision; a symlink: `Refusing to edit a symlinked deckhand.json; edit its target
/// explicitly`).
pub async fn read_document_at(root: &str) -> Result<DocumentAt> {
    let file = path::join(&[root, PROJECT_CONFIG_FILE]);
    let options = ReadBounded {
        no_follow: true,
        max: MAX_CONFIG_BYTES,
    };
    let document = |raw: String, revision: Option<String>, exists: bool| ProjectConfigDocument {
        kind: ConfigTargetKind::Repository,
        root: root.to_owned(),
        path: file.clone(),
        raw,
        revision,
        exists,
    };
    match read_bounded_utf8(Path::new(&file), options).await {
        Ok(read) => Ok(DocumentAt {
            document: document(read.text, Some(sha256(&read.bytes)), true),
            mode: read.mode,
        }),
        Err(error) if error.is_not_found() => Ok(DocumentAt {
            document: document(STARTER_PROJECT_CONFIG.to_owned(), None, false),
            mode: 0o644,
        }),
        Err(error) if error.is_code("ELOOP") => Err(DhError::msg(
            "Refusing to edit a symlinked deckhand.json; edit its target explicitly",
        )),
        Err(error) => Err(error),
    }
}

/// TS: projectConfigDocument.ts readProjectConfigDocument (always the main checkout's).
pub async fn read_project_config_document(cwd: &str) -> Result<ProjectConfigDocument> {
    let context = repository_context(cwd).await?;
    let root = context.main_root.unwrap_or_default();
    Ok(read_document_at(&root).await?.document)
}

/// Whether a hard link failed because the filesystem has none (TS: EPERM, ENOTSUP, EOPNOTSUPP, EXDEV, ENOSYS).
fn no_hard_links(error: &std::io::Error) -> bool {
    matches!(
        error.raw_os_error(),
        Some(libc::EPERM | libc::ENOTSUP | libc::EOPNOTSUPP | libc::EXDEV | libc::ENOSYS)
    )
}

/// `fs.copyFile(source, destination, COPYFILE_EXCL)`: never overwrites; the copy keeps the source's mode.
fn copy_exclusive(source: &str, destination: &str) -> std::io::Result<()> {
    let mut input = std::fs::File::open(source)?;
    let mode = input.metadata()?.permissions().mode() & 0o7777;
    let mut output = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .open(destination)?;
    std::io::copy(&mut input, &mut output)?;
    output.set_permissions(std::fs::Permissions::from_mode(mode))?;
    Ok(())
}

/// TS: projectConfigDocument.ts createExclusive (a hard link that never overwrites; an exclusive copy where the
/// filesystem has no hard links).
pub async fn create_exclusive(temporary: &Path, file: &Path) -> Result<()> {
    let (source, destination) = (
        temporary.to_string_lossy().into_owned(),
        file.to_string_lossy().into_owned(),
    );
    match tokio::fs::hard_link(&source, &destination).await {
        Ok(()) => Ok(()),
        Err(error) if no_hard_links(&error) => {
            let (from, to) = (source.clone(), destination.clone());
            tokio::task::spawn_blocking(move || copy_exclusive(&from, &to))
                .await
                .map_err(|error| DhError::msg(error.to_string()))?
                .map_err(|error| DhError::io_dest(&error, "copyfile", &source, &destination))
        }
        Err(error) => Err(DhError::io_dest(&error, "link", &source, &destination)),
    }
}

/// TS: projectConfigDocument.ts saveProjectConfigDocument options.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SaveDocumentOptions {
    /// A save made through Deckhand: decide trust from exactly the replaced bytes (see `config::saved_project_trust`)
    /// and trust the new bytes when allowed.
    pub keep_trust: bool,
}

thread_local! {
    /// TS `writes`: saves of one deckhand.json run one at a time (per process; one LocalSet per thread).
    static WRITES: KeyedQueue<String> = KeyedQueue::new();
}

const CHANGED_ON_DISK: &str =
    "deckhand.json changed on disk. Your draft is kept; close and reopen the editor before saving.";

/// The revision check, run before and again right before the file is replaced.
async fn check_revision(root: &str, expected_revision: Option<&str>) -> Result<DocumentAt> {
    let current = read_document_at(root).await?;
    if current.document.revision.as_deref() != expected_revision {
        return Err(DhError::msg(CHANGED_ON_DISK));
    }
    Ok(current)
}

/// The serialized part of a save: revision check, trust decision, write, trust update.
async fn write_document(
    context: RepoContext,
    raw: String,
    expected_revision: Option<String>,
    options: SaveDocumentOptions,
) -> Result<SavedConfigDocument> {
    let root = context.main_root.clone().unwrap_or_default();
    let file = path::join(&[&root, PROJECT_CONFIG_FILE]);
    let expected = expected_revision.as_deref();
    let current = check_revision(&root, expected).await?;
    // Decided from the bytes the revision names (what the user edited from), before anything is written.
    let decision = if options.keep_trust {
        let user = load_app_config().await?;
        let previous = PreviousDocument {
            raw: current.document.raw.clone(),
            exists: current.document.exists,
        };
        Some(saved_project_trust(&context, &previous, &raw, &user).await?)
    } else {
        None
    };
    let temporary = path::join(&[&root, &format!(".deckhand.json.{}.tmp", uuid::Uuid::new_v4())]);
    let result = async {
        let mut handle = tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(current.mode)
            .open(&temporary)
            .await
            .map_err(|error| DhError::io(&error, "open", &temporary))?;
        tokio::io::AsyncWriteExt::write_all(&mut handle, raw.as_bytes())
            .await
            .map_err(|error| DhError::io(&error, "write", &temporary))?;
        tokio::io::AsyncWriteExt::flush(&mut handle)
            .await
            .map_err(|error| DhError::io(&error, "write", &temporary))?;
        drop(handle);
        if current.document.exists {
            tokio::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(current.mode))
                .await
                .map_err(|error| DhError::io(&error, "chmod", &temporary))?;
        }
        check_revision(&root, expected).await?;
        if expected.is_none() {
            create_exclusive(Path::new(&temporary), Path::new(&file)).await?;
        } else {
            tokio::fs::rename(&temporary, &file)
                .await
                .map_err(|error| DhError::io_dest(&error, "rename", &temporary, &file))?;
        }
        // The fingerprint covers the bytes written here: anything written over them later needs a review again.
        if let Some(fingerprint) = decision.as_ref().and_then(|decision| decision.fingerprint.clone()) {
            let trust_root = context.trust_root.clone();
            update_app_config(move |mut config| {
                config.trusted_projects = Some(trust_project_config(
                    &trust_root,
                    &fingerprint,
                    config.trusted_projects.as_ref(),
                ));
                Ok(config)
            })
            .await?;
        }
        Ok(SavedConfigDocument {
            document: ProjectConfigDocument {
                kind: ConfigTargetKind::Repository,
                root: root.clone(),
                path: file.clone(),
                raw: raw.clone(),
                revision: Some(sha256(raw.as_bytes())),
                exists: true,
            },
            trust: decision.as_ref().map(|decision| decision.trust),
        })
    }
    .await;
    let _ = tokio::fs::remove_file(&temporary).await;
    result
}

/// TS: projectConfigDocument.ts saveProjectConfigDocument (writes the repository's deckhand.json if it still has
/// `expected_revision` (`None`: must not exist), else `deckhand.json changed on disk. Your draft is kept; close and
/// reopen the editor before saving.`; never replaces a valid file with an invalid draft; serialized per file).
pub async fn save_project_config_document(
    cwd: &str,
    raw: &str,
    expected_revision: Option<&str>,
    options: SaveDocumentOptions,
) -> Result<SavedConfigDocument> {
    check_draft(raw, expected_revision)?;
    // Never replace a valid file with an invalid draft.
    parse_project_config(raw, None)?;
    let context = repository_context(cwd).await?;
    let file = path::join(&[context.main_root.as_deref().unwrap_or_default(), PROJECT_CONFIG_FILE]);
    let (raw, expected) = (raw.to_owned(), expected_revision.map(str::to_owned));
    let task = WRITES.with(|writes| writes.run(file, move || write_document(context, raw, expected, options)));
    task.await
}

fn config_dir() -> String {
    paths::config_dir().to_string_lossy().into_owned()
}

/// TS: projectConfigDocument.ts readGlobalDefaultsDocument.
pub async fn read_global_defaults_document() -> Result<ProjectConfigDocument> {
    let config = load_app_config().await?;
    Ok(global_document(config.defaults.as_ref(), &config_dir()))
}

/// TS: projectConfigDocument.ts saveGlobalDefaultsDocument (validated as `defaults`; `Global defaults changed on disk.
/// Your draft is kept; close and reopen the editor before saving.` when the stored value's revision differs).
pub async fn save_global_defaults_document(
    raw: &str,
    expected_revision: Option<&str>,
) -> Result<ProjectConfigDocument> {
    check_draft(raw, expected_revision)?;
    let defaults = serde_json::to_value(parse_project_config(raw, Some("defaults"))?)?;
    let expected = expected_revision.map(str::to_owned);
    let stored = defaults.clone();
    update_app_config(move |current| {
        if global_revision(current.defaults.as_ref()) != expected {
            return Err(DhError::msg(
                "Global defaults changed on disk. Your draft is kept; close and reopen the editor before saving.",
            ));
        }
        Ok(deckhand_protocol::records::AppConfig {
            defaults: Some(stored),
            ..current
        })
    })
    .await?;
    Ok(global_document(Some(&defaults), &config_dir()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::load_project_config;
    use crate::config::tests::{UNIT, exists, real, replay_dir_cases, repo, tgit};
    use crate::storage::tests::{Home, block_on};
    use crate::storage::{load_app_config, patch_app_config};
    use deckhand_core::project_config::{UserSettings, is_project_trusted, resolve_settings};
    use deckhand_parity::{Case, DirOp, Replay};
    use deckhand_protocol::records::AppConfig;
    use serde_json::{Value, json};

    fn optional_str(case: &Case, index: usize) -> Option<String> {
        case.arg(index).as_str().map(str::to_owned)
    }

    fn global_op(op: &DirOp) -> std::result::Result<Value, String> {
        let outcome = match op.fn_name.as_str() {
            "readGlobalDefaultsDocument" => block_on(read_global_defaults_document()),
            "saveGlobalDefaultsDocument" => {
                let expected = optional_str(op, 1);
                block_on(save_global_defaults_document(op.arg_str(0)?, expected.as_deref()))
            }
            other => return Err(format!("parity: unknown op {other}")),
        };
        outcome
            .map(|document| serde_json::to_value(document).unwrap_or(Value::Null))
            .map_err(|error| error.to_string())
    }

    #[test]
    fn global_defaults_dir_cases_parity() {
        replay_dir_cases("projectConfigDocument-I6", |root, op| {
            let _home = Home::set(root);
            global_op(op).into()
        });
    }

    #[test]
    fn rejected_repository_drafts_parity() {
        let report = Replay::new(UNIT, "projectConfigDocument-I6/saveProjectConfigDocument").run(|case: &Case| {
            let options = SaveDocumentOptions {
                keep_trust: case.arg(3).get("keepTrust").and_then(Value::as_bool).unwrap_or(false),
            };
            let expected = optional_str(case, 2);
            let saved = block_on(save_project_config_document(
                case.arg_str(0)?,
                case.arg_str(1)?,
                expected.as_deref(),
                options,
            ));
            saved
                .map(|document| serde_json::to_value(document).unwrap_or(Value::Null))
                .map_err(|error| error.to_string())
        });
        report.assert_ok();
    }

    fn deckhand_json(cwd: &str) -> String {
        Path::new(cwd).join("deckhand.json").to_string_lossy().into_owned()
    }

    fn leftovers(cwd: &str) -> Vec<String> {
        std::fs::read_dir(cwd)
            .unwrap()
            .filter_map(|entry| entry.ok().map(|entry| entry.file_name().to_string_lossy().into_owned()))
            .filter(|name| name.starts_with(".deckhand.json."))
            .collect()
    }

    fn mode(file: &str) -> u32 {
        std::fs::metadata(file).unwrap().permissions().mode() & 0o777
    }

    // covers: B-I6-10 opening a missing config is side-effect free; explicit save creates a valid starter
    #[test]
    fn missing_config_opens_without_side_effects() {
        let (_dir, cwd) = repo();
        block_on(async {
            let document = read_project_config_document(&cwd).await.unwrap();
            assert!(!document.exists);
            assert_eq!(document.revision, None);
            assert_eq!(document.raw, STARTER_PROJECT_CONFIG);
            assert!(!exists(Path::new(&document.path)));
            let saved = save_project_config_document(
                &cwd,
                &document.raw,
                document.revision.as_deref(),
                SaveDocumentOptions::default(),
            )
            .await
            .unwrap();
            assert!(saved.document.exists);
            assert_eq!(
                saved.document.raw,
                std::fs::read_to_string(&saved.document.path).unwrap()
            );
            let project = load_project_config(&cwd, None, None).await.unwrap();
            assert_eq!(
                project.config.default_workspace,
                Some(deckhand_protocol::data::WorktreeMode::New)
            );
        });
    }

    // covers: B-I6-11 malformed JSON can be opened/repaired; invalid drafts never overwrite it
    #[test]
    fn malformed_json_can_be_repaired() {
        let (_dir, cwd) = repo();
        let file = deckhand_json(&cwd);
        std::fs::write(&file, "{").unwrap();
        block_on(async {
            let document = read_project_config_document(&cwd).await.unwrap();
            assert_eq!(document.raw, "{");
            let revision = document.revision.as_deref();
            let options = SaveDocumentOptions::default();
            assert!(
                save_project_config_document(&cwd, "{", revision, options)
                    .await
                    .is_err()
            );
            let error = save_project_config_document(&cwd, "{\"unknown\":true}", revision, options)
                .await
                .unwrap_err();
            assert!(error.to_string().contains("Unknown"), "{error}");
            assert_eq!(std::fs::read_to_string(&file).unwrap(), "{");
            save_project_config_document(&cwd, "{\"actions\":{}}\n", revision, options)
                .await
                .unwrap();
            let project = load_project_config(&cwd, None, None).await.unwrap();
            assert_eq!(project.config.actions, Some(indexmap::IndexMap::new()));
        });
    }

    // covers: B-I6-12 saving keeps the file permissions
    #[test]
    fn saving_keeps_the_file_permissions() {
        let (_dir, cwd) = repo();
        let file = deckhand_json(&cwd);
        std::fs::write(&file, "{}\n").unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o660)).unwrap();
        block_on(async {
            let document = read_project_config_document(&cwd).await.unwrap();
            save_project_config_document(
                &cwd,
                "{\"defaultAgent\":\"pi\"}\n",
                document.revision.as_deref(),
                SaveDocumentOptions::default(),
            )
            .await
            .unwrap();
        });
        assert_eq!(mode(&file), 0o660);
    }

    // covers: B-I6-13 a stale draft cannot replace an externally changed, created or removed file
    #[test]
    fn stale_drafts_never_replace_the_file() {
        let (_dir, cwd) = repo();
        let options = SaveDocumentOptions::default();
        block_on(async {
            let missing = read_project_config_document(&cwd).await.unwrap();
            std::fs::write(&missing.path, "{}").unwrap();
            let error =
                save_project_config_document(&cwd, STARTER_PROJECT_CONFIG, missing.revision.as_deref(), options)
                    .await
                    .unwrap_err();
            assert_eq!(error.to_string(), CHANGED_ON_DISK);
            let document = read_project_config_document(&cwd).await.unwrap();
            std::fs::write(&document.path, "{\"defaultAgent\":\"pi\"}").unwrap();
            let error = save_project_config_document(&cwd, "{}", document.revision.as_deref(), options)
                .await
                .unwrap_err();
            assert!(error.to_string().contains("changed on disk"), "{error}");
            assert_eq!(
                std::fs::read_to_string(&document.path).unwrap(),
                "{\"defaultAgent\":\"pi\"}"
            );
            std::fs::remove_file(&document.path).unwrap();
            let error = save_project_config_document(&cwd, "{}", document.revision.as_deref(), options)
                .await
                .unwrap_err();
            assert!(error.to_string().contains("changed on disk"), "{error}");
            assert!(!exists(Path::new(&document.path)));
            assert!(leftovers(&cwd).is_empty());
        });
    }

    // covers: B-I6-14 simultaneous saves with one revision have exactly one winner
    #[test]
    fn simultaneous_saves_have_one_winner() {
        let (_dir, cwd) = repo();
        let options = SaveDocumentOptions::default();
        block_on(async {
            let document = read_project_config_document(&cwd).await.unwrap();
            let revision = document.revision.as_deref();
            let (first, second) = futures::join!(
                save_project_config_document(&cwd, "{\"defaultAgent\":\"pi\"}", revision, options),
                save_project_config_document(&cwd, "{\"defaultAgent\":\"codex\"}", revision, options),
            );
            assert_eq!([first.is_ok(), second.is_ok()].iter().filter(|ok| **ok).count(), 1);
            assert!(leftovers(&cwd).is_empty());
        });
    }

    // covers: B-I6-15 from a linked worktree, the editor opens and saves the main checkout's file
    #[test]
    fn linked_worktrees_edit_the_main_checkout_file() {
        let (_dir, cwd) = repo();
        let worktree = Path::new(&cwd).join("linked").to_string_lossy().into_owned();
        tgit(&cwd, &["worktree", "add", "-b", "linked", &worktree]);
        std::fs::write(deckhand_json(&worktree), "{\"devCommand\":\"printf ignored\"}").unwrap();
        block_on(async {
            let document = read_project_config_document(&worktree).await.unwrap();
            assert_eq!(document.path, deckhand_json(&real(Path::new(&cwd))));
            assert!(!document.exists);
            save_project_config_document(
                &worktree,
                "{\"devCommand\":\"printf shared\"}",
                document.revision.as_deref(),
                SaveDocumentOptions::default(),
            )
            .await
            .unwrap();
            let project = load_project_config(&worktree, None, None).await.unwrap();
            assert_eq!(project.config.dev_command_text(), Some("printf shared"));
        });
        assert_eq!(
            std::fs::read_to_string(deckhand_json(&worktree)).unwrap(),
            "{\"devCommand\":\"printf ignored\"}"
        );
    }

    // covers: B-I6-16 symlinked and oversized configs are rejected without modifying their targets
    #[test]
    fn symlinked_and_oversized_configs_are_rejected() {
        let (_dir, cwd) = repo();
        let (_target_dir, target) = repo();
        let (file, outside) = (deckhand_json(&cwd), deckhand_json(&target));
        std::fs::write(&outside, "{}").unwrap();
        std::os::unix::fs::symlink(&outside, &file).unwrap();
        let options = SaveDocumentOptions::default();
        block_on(async {
            let error = read_project_config_document(&cwd).await.unwrap_err();
            assert!(error.to_string().contains("symlink"), "{error}");
            let error = save_project_config_document(&cwd, "{}", None, options)
                .await
                .unwrap_err();
            assert!(error.to_string().contains("symlink"), "{error}");
            assert_eq!(std::fs::read_to_string(&outside).unwrap(), "{}");
            std::fs::remove_file(&file).unwrap();
            std::fs::write(&file, " ".repeat(65537)).unwrap();
            let error = read_project_config_document(&cwd).await.unwrap_err();
            assert!(error.to_string().contains("64 KiB"), "{error}");
            let error = save_project_config_document(&cwd, &" ".repeat(65537), None, options)
                .await
                .unwrap_err();
            assert!(error.to_string().contains("64 KiB"), "{error}");
        });
    }

    /// Whether the project at `cwd` is trusted by the stored user config.
    async fn trusted(cwd: &str) -> bool {
        let user = load_app_config().await.unwrap();
        let project = load_project_config(cwd, Some(&user), None).await.unwrap();
        is_project_trusted(
            &project.trust_root,
            &project.fingerprint,
            user.trusted_projects.as_ref(),
        )
    }

    /// The trust the save reports, saving over the current revision.
    async fn save_over(cwd: &str, raw: &str) -> Option<deckhand_protocol::data::SaveTrust> {
        let revision = read_project_config_document(cwd).await.unwrap().revision;
        let keep = SaveDocumentOptions { keep_trust: true };
        save_project_config_document(cwd, raw, revision.as_deref(), keep)
            .await
            .unwrap()
            .trust
    }

    async fn trust_now(cwd: &str) {
        let project = load_project_config(cwd, Some(&load_app_config().await.unwrap()), None)
            .await
            .unwrap();
        update_app_config(move |mut config| {
            config.trusted_projects = Some(trust_project_config(
                &project.trust_root,
                &project.fingerprint,
                config.trusted_projects.as_ref(),
            ));
            Ok(config)
        })
        .await
        .unwrap();
    }

    // covers: B-I6-17 saves through Deckhand keep a trusted (or new) deckhand.json trusted; unreviewed bytes, outside edits and creation hooks still need review
    #[test]
    fn deckhand_saves_keep_trust() {
        use deckhand_protocol::data::SaveTrust;
        let home = tempfile::Builder::new()
            .prefix("deckhand-save-trust-")
            .tempdir()
            .unwrap();
        let _home = Home::set(home.path());
        let (_dir, cwd) = repo();
        let (_other_dir, other) = repo();
        let keep = SaveDocumentOptions { keep_trust: true };
        block_on(async {
            // No file and no hook: nothing to review, so the created file is trusted; saving over trusted bytes keeps it.
            let absent = read_project_config_document(&cwd).await.unwrap();
            let saved = save_project_config_document(&cwd, "{\"devCommand\":\"one\"}\n", None, keep)
                .await
                .unwrap();
            assert_eq!(saved.trust, Some(SaveTrust::Created));
            assert!(trusted(&cwd).await);
            let saved = save_project_config_document(
                &cwd,
                "{\"devCommand\":\"two\"}\n",
                saved.document.revision.as_deref(),
                keep,
            )
            .await
            .unwrap();
            assert_eq!(saved.trust, Some(SaveTrust::Kept));
            assert!(trusted(&cwd).await);
            // A stale revision is still rejected, and grants nothing.
            let error =
                save_project_config_document(&cwd, "{\"devCommand\":\"stale\"}\n", absent.revision.as_deref(), keep)
                    .await
                    .unwrap_err();
            assert!(error.to_string().contains("changed on disk"), "{error}");
            // An edit from outside Deckhand needs review again; a Deckhand save over those unreviewed bytes still does.
            std::fs::write(&saved.document.path, "{\"devCommand\":\"outside\"}\n").unwrap();
            assert!(!trusted(&cwd).await);
            assert_eq!(
                save_over(&cwd, "{\"devCommand\":\"three\"}\n").await,
                Some(SaveTrust::Unreviewed)
            );
            assert!(!trusted(&cwd).await);
            // Without keepTrust (not a Deckhand edit) trust is untouched.
            update_app_config(|config| {
                Ok(AppConfig {
                    trusted_projects: None,
                    ..config
                })
            })
            .await
            .unwrap();
            let revision = read_project_config_document(&cwd).await.unwrap().revision;
            let saved = save_project_config_document(&cwd, "{}\n", revision.as_deref(), SaveDocumentOptions::default())
                .await
                .unwrap();
            assert_eq!(saved.trust, None);

            // Creation hook. Trusted without the hook (hook: false), a save that enables the hook needs review.
            let hook = Path::new(&cwd).join(".claude/scripts/create-worktree.sh");
            std::fs::create_dir_all(hook.parent().unwrap()).unwrap();
            std::fs::write(&hook, "#!/bin/bash\necho one\n").unwrap();
            let revision = read_project_config_document(&cwd).await.unwrap().revision;
            save_project_config_document(
                &cwd,
                "{\"worktree\":{\"hook\":false}}\n",
                revision.as_deref(),
                SaveDocumentOptions::default(),
            )
            .await
            .unwrap();
            trust_now(&cwd).await;
            assert!(trusted(&cwd).await);
            assert_eq!(
                save_over(&cwd, "{\"devCommand\":\"hooked\"}\n").await,
                Some(SaveTrust::Hook)
            );
            assert!(!trusted(&cwd).await);
            // Once the file and hook are trusted together, settings saves keep them trusted (the hook's bytes are
            // unchanged).
            trust_now(&cwd).await;
            assert_eq!(
                save_over(&cwd, "{\"devCommand\":\"four\"}\n").await,
                Some(SaveTrust::Kept)
            );
            assert!(trusted(&cwd).await);
            // The hook edited since that trust: the bundle needs review, and a settings save never trusts the new hook.
            std::fs::write(&hook, "#!/bin/bash\necho changed\n").unwrap();
            assert_eq!(
                save_over(&cwd, "{\"devCommand\":\"five\"}\n").await,
                Some(SaveTrust::Unreviewed)
            );
            assert!(!trusted(&cwd).await);
            // No file yet but an untrusted hook: creating the file does not trust the hook.
            let other_hook = Path::new(&other).join(".claude/scripts/create-worktree.sh");
            std::fs::create_dir_all(other_hook.parent().unwrap()).unwrap();
            std::fs::write(&other_hook, "echo x\n").unwrap();
            let saved = save_project_config_document(&other, "{}\n", None, keep).await.unwrap();
            assert_eq!(saved.trust, Some(SaveTrust::Hook));
            assert!(!trusted(&other).await);
        });
    }

    fn settings_json(user: &UserSettings, config_dir: &str) -> Value {
        serde_json::to_value(resolve_settings(None, user, config_dir).unwrap()).unwrap()
    }

    // covers: B-I6-18 global defaults round-trip through config.json, keep other keys, and are validated with a revision check
    #[test]
    fn global_defaults_round_trip() {
        let home = tempfile::tempdir().unwrap();
        let _home = Home::set(home.path());
        let config_dir = home.path().to_string_lossy().into_owned();
        let file = home.path().join("config.json");
        std::fs::write(
            &file,
            "{\"future_setting\":1,\"trustedProjects\":{\"/r\":[\"x\"]},\"dev_command\":\"legacy\"}",
        )
        .unwrap();
        let on_disk = || serde_json::from_str::<Value>(&std::fs::read_to_string(&file).unwrap()).unwrap();
        block_on(async {
            let starter = read_global_defaults_document().await.unwrap();
            assert!(!starter.exists);
            assert_eq!(starter.revision, None);
            let error = save_global_defaults_document("{\"setupCommand\":\"\"}", None)
                .await
                .unwrap_err();
            assert!(error.to_string().contains("Invalid command"), "{error}");
            let error = save_global_defaults_document("{\"unknown\":1}", None)
                .await
                .unwrap_err();
            assert!(error.to_string().contains("Unknown defaults setting"), "{error}");
            let saved = save_global_defaults_document(
                "{\"setupCommand\":\"npm ci\",\"actions\":{\"test\":\"npm test\"}}",
                None,
            )
            .await
            .unwrap();
            assert_eq!(
                on_disk(),
                json!({"future_setting": 1, "trustedProjects": {"/r": ["x"]}, "dev_command": "legacy",
                    "defaults": {"setupCommand": "npm ci", "actions": {"test": "npm test"}}})
            );
            assert_eq!(
                settings_json(&load_app_config().await.unwrap(), &config_dir),
                json!({"setupCommand": "npm ci", "devCommand": "legacy", "actions": {"test": "npm test"}})
            );
            // Unrelated updates keep the defaults and their revision.
            patch_app_config(AppConfig {
                notifications: Some(true),
                ..AppConfig::default()
            })
            .await
            .unwrap();
            assert_eq!(read_global_defaults_document().await.unwrap().revision, saved.revision);
            let error = save_global_defaults_document("{}", starter.revision.as_deref())
                .await
                .unwrap_err();
            assert!(error.to_string().contains("changed on disk"), "{error}");
            // Invalid defaults on disk surface where used and can still be opened for repair.
            update_app_config(|current| {
                Ok(AppConfig {
                    defaults: Some(json!({"defaultAgent": "nope"})),
                    ..current
                })
            })
            .await
            .unwrap();
            let invalid = load_app_config().await.unwrap();
            let error = resolve_settings(None, &invalid, &config_dir).unwrap_err();
            assert!(error.to_string().contains("Invalid \"defaults\""), "{error}");
            let broken = read_global_defaults_document().await.unwrap();
            assert!(broken.raw.contains("nope"));
            save_global_defaults_document("{}", broken.revision.as_deref())
                .await
                .unwrap();
            assert_eq!(
                settings_json(&load_app_config().await.unwrap(), &config_dir),
                json!({"devCommand": "legacy"})
            );
        });
    }
}
