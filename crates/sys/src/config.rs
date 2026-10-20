//! Port of projectConfig.ts (file I/O parts).
//!
//! Reading the repository override (the main checkout's live deckhand.json; copies inside linked worktrees are
//! ignored) together with the creation hook, which are trusted together, and the trust decision for a deckhand.json
//! saved through Deckhand. Validation, fingerprints and effective settings are pure and live in
//! `deckhand_core::project_config`.

use std::io::Read;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;

use deckhand_core::config_draft::MAX_CONFIG_BYTES;
use deckhand_core::error::{DhError, Result, errno_code, errno_description};
use deckhand_core::jsstr::path;
use deckhand_core::project_config::{
    PROJECT_CONFIG_FILE, UserSettings, hook_setting, is_project_trusted, parse_project_config, project_fingerprint,
    sha256,
};
use deckhand_protocol::Ordered;
use deckhand_protocol::data::{CreationHook, DisabledHook, LoadedProject, ProjectConfig, SaveTrust};

use crate::git::{RepoContext, resolve_create_script, resolve_repo_context};

/// TS: projectConfig.ts readBoundedUtf8 result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BoundedFile {
    /// The bytes read (a BOM kept, so the text round-trips to the hashed bytes).
    pub bytes: Vec<u8>,
    pub text: String,
    /// Permission bits (`mode & 0o777`).
    pub mode: u32,
}

/// TS: projectConfig.ts readBoundedUtf8 options.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReadBounded {
    /// Refuse a symlink (`O_NOFOLLOW`: `<name> must not be a symlink`, code `ELOOP`).
    pub no_follow: bool,
    /// Bytes allowed at most (`<name> exceeds <max/1024 rounded> KiB`).
    pub max: usize,
}

/// An fs error from a call on an open handle: Node names the syscall but not the path (`EISDIR: ..., read`).
fn handle_error(error: &std::io::Error, syscall: &str) -> DhError {
    match errno_code(error) {
        Some(code) => DhError::with_code(code, format!("{code}: {}, {syscall}", errno_description(code))),
        None => DhError::msg(error.to_string()),
    }
}

fn read_bounded_blocking(file: &str, options: ReadBounded) -> Result<BoundedFile> {
    let name = path::basename(file);
    #[allow(clippy::cast_precision_loss)]
    let kib = (options.max as f64 / 1024.0).round();
    let limit = || DhError::msg(format!("{name} exceeds {kib} KiB"));
    // Non-blocking, so opening a FIFO never hangs.
    let mut flags = libc::O_NONBLOCK;
    if options.no_follow {
        flags |= libc::O_NOFOLLOW;
    }
    let mut handle = match std::fs::OpenOptions::new().read(true).custom_flags(flags).open(file) {
        Ok(handle) => handle,
        Err(error) => {
            let error = DhError::io(&error, "open", file);
            if error.is_code("ELOOP") && options.no_follow {
                return Err(DhError::with_code("ELOOP", format!("{name} must not be a symlink")));
            }
            return Err(error);
        }
    };
    let stat = handle.metadata().map_err(|error| handle_error(&error, "fstat"))?;
    if !stat.is_file() {
        return Err(DhError::msg(format!("{name} must be a regular file")));
    }
    if stat.len() > options.max as u64 {
        return Err(limit());
    }
    // One byte more than allowed tells a file that grew since the stat.
    let mut buffer = vec![0u8; options.max.saturating_add(1)];
    let mut length = 0;
    while length < buffer.len() {
        let read = match handle.read(&mut buffer[length..]) {
            Ok(read) => read,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(handle_error(&error, "read")),
        };
        if read == 0 {
            break;
        }
        length += read;
    }
    if length > options.max {
        return Err(limit());
    }
    buffer.truncate(length);
    let text = String::from_utf8(buffer.clone()).map_err(|_| {
        DhError::with_code(
            "ERR_ENCODING_INVALID_ENCODED_DATA",
            "The encoded data was not valid for encoding utf-8",
        )
    })?;
    Ok(BoundedFile {
        bytes: buffer,
        text,
        mode: stat.mode() & 0o777,
    })
}

/// TS: projectConfig.ts readBoundedUtf8 (strict UTF-8; a non-blocking open so a FIFO never hangs; `<name> must be a
/// regular file`; fs errors keep their code, e.g. `ENOENT`).
pub async fn read_bounded_utf8(file: &Path, options: ReadBounded) -> Result<BoundedFile> {
    let file = file.to_string_lossy().into_owned();
    tokio::task::spawn_blocking(move || read_bounded_blocking(&file, options))
        .await
        .map_err(|error| DhError::msg(error.to_string()))?
}

fn bounded(max: usize) -> ReadBounded {
    ReadBounded { no_follow: false, max }
}

/// TS: projectConfig.ts loadProjectConfig: the main checkout's deckhand.json (errors prefixed `<file>: `) and the
/// creation hook (enabled unless `worktree.hook: false` in the file or the global defaults; read, fingerprinted).
/// `user` is the user config (default empty): every caller that compares fingerprints must pass the same one.
/// `context`: the repository context when the caller has it (else resolved from `cwd`).
pub async fn load_project_config(
    cwd: &str,
    user: Option<&UserSettings>,
    context: Option<&RepoContext>,
) -> Result<LoadedProject> {
    let default_user = UserSettings::default();
    let user = user.unwrap_or(&default_user);
    let context = match context {
        Some(context) => context.clone(),
        None => resolve_repo_context(cwd).await?,
    };
    let RepoContext {
        root,
        main_root,
        trust_root,
        ..
    } = context;
    let file = main_root
        .as_deref()
        .map(|main| path::join(&[main, PROJECT_CONFIG_FILE]));
    let mut raw = String::new();
    let mut exists = false;
    if let Some(file) = &file {
        let options = ReadBounded {
            no_follow: true,
            max: MAX_CONFIG_BYTES,
        };
        match read_bounded_utf8(Path::new(file), options).await {
            Ok(read) => {
                raw = read.text;
                exists = true;
            }
            Err(error) if error.is_not_found() => {}
            Err(error) => return Err(error.prefixed(&format!("{file}: "))),
        }
    }
    let config: Ordered<ProjectConfig> = if exists {
        parse_project_config(&raw, None)
            .map_err(|error| error.prefixed(&format!("{}: ", file.as_deref().unwrap_or("undefined"))))?
    } else {
        Ordered::default()
    };
    let hook_path = resolve_create_script(&root, main_root.as_deref()).await;
    let mut creation_hook = None;
    let mut disabled_hook = None;
    if let Some(hook_path) = hook_path {
        let setting = hook_setting(&config, user);
        match setting.by {
            Some(by) if !setting.enabled => disabled_hook = Some(DisabledHook { file: hook_path, by }),
            _ => {
                let read = read_bounded_utf8(Path::new(&hook_path), bounded(MAX_CONFIG_BYTES)).await?;
                creation_hook = Some(CreationHook {
                    file: hook_path,
                    content: read.text,
                    fingerprint: sha256(&read.bytes),
                });
            }
        }
    }
    let fingerprint = project_fingerprint(&raw, creation_hook.as_ref().map(|hook| hook.fingerprint.as_str()));
    Ok(LoadedProject {
        root,
        trust_root,
        path: file,
        config,
        exists,
        fingerprint,
        creation_hook,
        disabled_hook,
    })
}

/// The bytes a save replaced (TS savedProjectTrust `previous`): exactly what the user edited from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreviousDocument {
    pub raw: String,
    pub exists: bool,
}

/// TS: projectConfig.ts savedProjectTrust result: whether the new bytes stay (or become) trusted, and the fingerprint
/// to add to the trusted list when they do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SavedTrust {
    pub trust: SaveTrust,
    pub fingerprint: Option<String>,
}

/// TS: projectConfig.ts savedProjectTrust (self-edits keep trust only when the whole previous bundle was trusted and
/// the creation hook does not join the bundle through this save).
///
/// The new bytes are trusted only when (1) `previous` together with the creation hook as it is now (enabled or not
/// per `previous` and the global defaults) was trusted, and (2) `next` does not enable a hook `previous` did not: a
/// hook is only ever trusted through the review.
pub async fn saved_project_trust(
    context: &RepoContext,
    previous: &PreviousDocument,
    next: &str,
    user: &UserSettings,
) -> Result<SavedTrust> {
    let hook = match resolve_create_script(&context.root, context.main_root.as_deref()).await {
        Some(hook_path) => Some(sha256(
            &read_bounded_utf8(Path::new(&hook_path), bounded(MAX_CONFIG_BYTES))
                .await?
                .bytes,
        )),
        None => None,
    };
    let before = if previous.exists {
        match parse_project_config(&previous.raw, None) {
            Ok(config) => config,
            // Never loadable, so never trusted.
            Err(_) => {
                return Ok(SavedTrust {
                    trust: SaveTrust::Unreviewed,
                    fingerprint: None,
                });
            }
        }
    } else {
        Ordered::default()
    };
    let enabled = |config: &ProjectConfig| hook.is_some() && hook_setting(config, user).enabled;
    let hook_before = enabled(&before);
    let hook_after = enabled(&*parse_project_config(next, None)?);
    let trusted = if !previous.exists && !hook_before {
        true
    } else {
        let raw = if previous.exists { previous.raw.as_str() } else { "" };
        let fingerprint = project_fingerprint(raw, if hook_before { hook.as_deref() } else { None });
        is_project_trusted(&context.trust_root, &fingerprint, user.trusted_projects.as_ref())
    };
    if !trusted {
        let trust = if previous.exists {
            SaveTrust::Unreviewed
        } else {
            SaveTrust::Hook
        };
        return Ok(SavedTrust {
            trust,
            fingerprint: None,
        });
    }
    if hook_after && !hook_before {
        return Ok(SavedTrust {
            trust: SaveTrust::Hook,
            fingerprint: None,
        });
    }
    Ok(SavedTrust {
        trust: if previous.exists {
            SaveTrust::Kept
        } else {
            SaveTrust::Created
        },
        fingerprint: Some(project_fingerprint(
            next,
            if hook_after { hook.as_deref() } else { None },
        )),
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::git::create_worktree_for_session;
    use crate::storage::tests::{Home, block_on};
    use deckhand_core::project_config::{project_needs_review, resolve_settings, trust_project_config};
    use deckhand_parity::{DirOp, Outcome, Replay, dir_cases, tags};
    use deckhand_protocol::data::{ConfigTargetKind, WorktreeCreator};
    use serde_json::{Value, json};
    use std::process::Command;

    pub(crate) const UNIT: &str = "I6";

    /// Replays every directory case of `module` listed in the manifest; fails with every failing case's summary.
    pub(crate) fn replay_dir_cases(module: &str, mut port: impl FnMut(&Path, &DirOp) -> Outcome) {
        let manifest = deckhand_parity::Manifest::load(UNIT).expect("manifest");
        let cases: Vec<String> = dir_cases(module)
            .into_iter()
            .filter(|case| manifest.fixtures.contains(case))
            .collect();
        assert!(!cases.is_empty(), "no directory cases for {module}");
        let mut failed = Vec::new();
        for fixture in cases {
            let report = Replay::new(UNIT, &fixture).run_dir(&mut port);
            if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| report.assert_ok())).is_err() {
                failed.push(report.summary());
            }
        }
        assert!(failed.is_empty(), "{}", failed.join("\n\n"));
    }

    /// tests/helpers.ts `git(cwd, ...args)`: trimmed stdout; panics on failure. Ignores the user's Git config.
    pub(crate) fn tgit(dir: &str, args: &[&str]) -> String {
        let output = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .expect("git runs");
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    }

    pub(crate) fn real(dir: &Path) -> String {
        std::fs::canonicalize(dir).unwrap().to_string_lossy().into_owned()
    }

    /// tests/helpers.ts `repo()`: a repository on `main` with one commit (file.txt).
    pub(crate) fn repo() -> (tempfile::TempDir, String) {
        let dir = tempfile::Builder::new().prefix("deckhand-test-").tempdir().unwrap();
        let path = real(dir.path());
        tgit(&path, &["init", "-b", "main"]);
        tgit(&path, &["config", "user.name", "Deckhand Test"]);
        tgit(&path, &["config", "user.email", "deckhand@example.invalid"]);
        tgit(&path, &["config", "commit.gpgsign", "false"]);
        let no_hooks = Path::new(&path).join(".git").join("no-hooks");
        tgit(&path, &["config", "core.hooksPath", &no_hooks.to_string_lossy()]);
        std::fs::write(Path::new(&path).join("file.txt"), "first\n").unwrap();
        tgit(&path, &["add", "."]);
        tgit(&path, &["commit", "-m", "initial"]);
        (dir, path)
    }

    pub(crate) fn exists(path: &Path) -> bool {
        std::fs::symlink_metadata(path).is_ok()
    }

    fn context_arg(value: &Value) -> RepoContext {
        let field = |key: &str| value.get(key).and_then(Value::as_str).map(str::to_owned);
        RepoContext {
            root: field("root").unwrap_or_default(),
            common_dir: String::new(),
            main_root: field("mainRoot"),
            trust_root: field("trustRoot").unwrap_or_default(),
        }
    }

    fn user_arg(op: &DirOp, index: usize) -> std::result::Result<UserSettings, String> {
        if op.arg_is_undefined(index) {
            Ok(UserSettings::default())
        } else {
            op.arg_as(index)
        }
    }

    fn project_config_op(op: &DirOp) -> std::result::Result<Value, String> {
        let outcome = match op.fn_name.as_str() {
            "readBoundedUtf8" => {
                let options = op.arg(1);
                let options = ReadBounded {
                    no_follow: options.get("noFollow").and_then(Value::as_bool).unwrap_or(false),
                    max: options
                        .get("max")
                        .and_then(Value::as_u64)
                        .and_then(|max| usize::try_from(max).ok())
                        .ok_or("parity: max")?,
                };
                block_on(read_bounded_utf8(Path::new(op.arg_str(0)?), options))
                    .map(|read| json!({"bytes": tags::bytes(&read.bytes), "text": read.text, "mode": read.mode}))
            }
            "loadProjectConfig" => {
                let user = user_arg(op, 1)?;
                let context = context_arg(op.arg(2));
                block_on(load_project_config(op.arg_str(0)?, Some(&user), Some(&context)))
                    .map(|project| serde_json::to_value(project).unwrap_or(Value::Null))
            }
            "savedProjectTrust" => {
                let context = context_arg(op.arg(0));
                let previous = op.arg(1);
                let previous = PreviousDocument {
                    raw: previous
                        .get("raw")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                    exists: previous.get("exists").and_then(Value::as_bool).unwrap_or(false),
                };
                let user = user_arg(op, 3)?;
                block_on(saved_project_trust(&context, &previous, op.arg_str(2)?, &user)).map(|saved| {
                    let mut value = json!({"trust": saved.trust});
                    if let Some(fingerprint) = saved.fingerprint {
                        value["fingerprint"] = json!(fingerprint);
                    }
                    value
                })
            }
            other => return Err(format!("parity: unknown op {other}")),
        };
        outcome.map_err(|error| error.to_string())
    }

    #[test]
    fn project_config_dir_cases_parity() {
        replay_dir_cases("projectConfig-I6", |_root, op| project_config_op(op).into());
    }

    #[test]
    fn read_bounded_utf8_never_hangs_on_a_fifo() {
        let dir = tempfile::tempdir().unwrap();
        let fifo = dir.path().join("deckhand.json");
        nix::unistd::mkfifo(&fifo, nix::sys::stat::Mode::from_bits_truncate(0o600)).unwrap();
        let error = block_on(read_bounded_utf8(&fifo, bounded(1024))).unwrap_err();
        assert_eq!(error.to_string(), "deckhand.json must be a regular file");
    }

    const HOOK_SCRIPT: &str = "#!/bin/bash\ntouch \"$CLAUDE_PROJECT_DIR/hook-ran\"\nexit 1\n";

    // covers: B-I6-01 legacy creation hooks are inspectable, content-trusted and never run by default
    #[test]
    fn legacy_creation_hooks_are_content_trusted() {
        let (_dir, cwd) = repo();
        let home = tempfile::Builder::new()
            .prefix("deckhand-hook-trust-")
            .tempdir()
            .unwrap();
        let _home = Home::set(home.path());
        block_on(async {
            let file = Path::new(&cwd).join(".claude/scripts/create-worktree.sh");
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(&file, HOOK_SCRIPT).unwrap();
            let project = load_project_config(&cwd, None, None).await.unwrap();
            assert!(project.creation_hook.as_ref().unwrap().content.contains("hook-ran"));
            let trusted = trust_project_config(&project.trust_root, &project.fingerprint, None);
            let worktree = create_worktree_for_session("untrusted", &cwd, None, None, None)
                .await
                .unwrap();
            assert_eq!(worktree.creator, WorktreeCreator::Fallback);
            assert!(worktree.path.starts_with(&real(home.path())), "{}", worktree.path);
            assert!(!exists(&Path::new(&cwd).join("hook-ran")));
            let error = create_worktree_for_session("approved", &cwd, project.creation_hook.as_ref(), None, None)
                .await
                .unwrap_err();
            assert!(error.to_string().contains("failed with exit code"), "{error}");
            assert!(exists(&Path::new(&cwd).join("hook-ran")));
            std::fs::remove_file(Path::new(&cwd).join("hook-ran")).unwrap();
            // Swapping the file after review invalidates trust and cannot change what runs: only the reviewed bytes
            // execute.
            std::fs::write(&file, "#!/bin/bash\ntouch \"$CLAUDE_PROJECT_DIR/swapped\"\nexit 1\n").unwrap();
            let swapped = load_project_config(&cwd, None, None).await.unwrap();
            assert!(!is_project_trusted(
                &swapped.trust_root,
                &swapped.fingerprint,
                Some(&trusted)
            ));
            let error = create_worktree_for_session("verified", &cwd, project.creation_hook.as_ref(), None, None)
                .await
                .unwrap_err();
            assert!(error.to_string().contains("failed with exit code"), "{error}");
            assert!(exists(&Path::new(&cwd).join("hook-ran")));
            assert!(!exists(&Path::new(&cwd).join("swapped")));
            std::fs::remove_file(Path::new(&cwd).join("hook-ran")).unwrap();
        });
    }

    fn user(value: Value) -> UserSettings {
        serde_json::from_value(value).unwrap()
    }

    // covers: B-I6-05 worktree.hook false: the hook never runs, is not fingerprinted or reviewed; untrusted false disables, untrusted true never enables
    #[test]
    fn worktree_hook_false_disables_the_hook() {
        let (_dir, cwd) = repo();
        let home = tempfile::Builder::new().prefix("deckhand-hook-off-").tempdir().unwrap();
        let _home = Home::set(home.path());
        let config_dir = real(home.path());
        block_on(async {
            let bare = load_project_config(&cwd, None, None).await.unwrap();
            let file = Path::new(&cwd).join(".claude/scripts/create-worktree.sh");
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(&file, HOOK_SCRIPT).unwrap();
            let empty = UserSettings::default();
            let loaded = load_project_config(&cwd, None, None).await.unwrap();
            assert!(project_needs_review(&loaded, None));
            // Global defaults switch it off: nothing to trust, the same fingerprint as no hook, and creation falls back.
            let off = user(json!({"defaults": {"worktree": {"hook": false}}}));
            let global_off = load_project_config(&cwd, Some(&off), None).await.unwrap();
            assert_eq!(global_off.creation_hook, None);
            assert_eq!(
                global_off.disabled_hook,
                Some(DisabledHook {
                    file: real(&file),
                    by: ConfigTargetKind::Global
                })
            );
            assert_eq!(global_off.fingerprint, bare.fingerprint);
            assert!(!project_needs_review(&global_off, off.trusted_projects.as_ref()));
            let created = create_worktree_for_session("no hook", &cwd, global_off.creation_hook.as_ref(), None, None)
                .await
                .unwrap();
            assert_eq!(created.creator, WorktreeCreator::Fallback);
            assert!(!exists(&Path::new(&cwd).join("hook-ran")));
            // An untrusted repository may switch it off...
            std::fs::write(Path::new(&cwd).join("deckhand.json"), "{\"worktree\":{\"hook\":false}}").unwrap();
            let repo_off = load_project_config(&cwd, None, None).await.unwrap();
            assert_eq!(repo_off.creation_hook, None);
            assert_eq!(
                repo_off.disabled_hook.as_ref().map(|hook| hook.by),
                Some(ConfigTargetKind::Repository)
            );
            assert!(!is_project_trusted(&repo_off.trust_root, &repo_off.fingerprint, None));
            let settings = resolve_settings(Some(&repo_off), &empty, &config_dir).unwrap();
            assert_eq!(
                settings.worktree.as_ref().and_then(|worktree| worktree.hook),
                Some(false)
            );
            // ...but never on: with global false, an untrusted hook: true is reviewed with the hook and not effective
            // until trusted.
            std::fs::write(Path::new(&cwd).join("deckhand.json"), "{\"worktree\":{\"hook\":true}}").unwrap();
            let repo_on = load_project_config(&cwd, Some(&off), None).await.unwrap();
            assert!(repo_on.creation_hook.is_some());
            assert!(project_needs_review(&repo_on, off.trusted_projects.as_ref()));
            let settings = resolve_settings(Some(&repo_on), &off, &config_dir).unwrap();
            assert_eq!(
                settings.worktree.as_ref().and_then(|worktree| worktree.hook),
                Some(false)
            );
            let trusted = UserSettings {
                trusted_projects: Some(trust_project_config(
                    &repo_on.trust_root,
                    &repo_on.fingerprint,
                    off.trusted_projects.as_ref(),
                )),
                ..off.clone()
            };
            let settings = resolve_settings(Some(&repo_on), &trusted, &config_dir).unwrap();
            assert_eq!(
                settings.worktree.as_ref().and_then(|worktree| worktree.hook),
                Some(true)
            );
        });
    }

    /// `trustProjectConfig(project, config)` on a user config.
    fn trusting(project: &LoadedProject, config: &UserSettings) -> UserSettings {
        UserSettings {
            trusted_projects: Some(trust_project_config(
                &project.trust_root,
                &project.fingerprint,
                config.trusted_projects.as_ref(),
            )),
            ..config.clone()
        }
    }

    fn trusted_by(project: &LoadedProject, config: &UserSettings) -> bool {
        is_project_trusted(
            &project.trust_root,
            &project.fingerprint,
            config.trusted_projects.as_ref(),
        )
    }

    // covers: B-I6-06 the repository override is the main checkout's live deckhand.json; worktree copies are ignored
    #[test]
    fn the_main_checkout_file_is_the_override() {
        let (_dir, cwd) = repo();
        let (_other_dir, other) = repo();
        let home = tempfile::tempdir().unwrap();
        let config_dir = real(home.path());
        let dev = |project: &LoadedProject, user: &UserSettings| {
            deckhand_core::project_config::resolve_dev_command(Some(project), user, &config_dir).unwrap()
        };
        block_on(async {
            let empty = UserSettings::default();
            // Nothing to trust.
            assert!(!project_needs_review(
                &load_project_config(&other, None, None).await.unwrap(),
                None
            ));
            let raw = "{\"devCommand\":\"touch DO_NOT_EXECUTE\",\"defaultWorkspace\":\"new\"}";
            std::fs::write(Path::new(&cwd).join("deckhand.json"), raw).unwrap();
            std::fs::write(Path::new(&other).join("deckhand.json"), raw).unwrap();
            tgit(&cwd, &["add", "deckhand.json"]);
            tgit(&cwd, &["commit", "-m", "project config"]);
            let worktree = Path::new(&cwd).join("linked").to_string_lossy().into_owned();
            tgit(&cwd, &["worktree", "add", "-b", "linked", &worktree]);
            let project = load_project_config(&worktree, None, None).await.unwrap();
            assert_eq!(
                project.path,
                Some(path::join(&[&real(Path::new(&cwd)), "deckhand.json"]))
            );
            assert!(project_needs_review(&project, empty.trusted_projects.as_ref()));
            let global = user(json!({"dev_command": "global"}));
            assert_eq!(dev(&project, &global), "global");
            let config = trusting(&project, &global);
            assert_eq!(dev(&project, &config), "touch DO_NOT_EXECUTE");
            // Same repository from any checkout.
            assert!(trusted_by(
                &load_project_config(&cwd, None, None).await.unwrap(),
                &config
            ));
            assert!(!trusted_by(
                &load_project_config(&other, None, None).await.unwrap(),
                &config
            ));
            std::fs::write(
                Path::new(&worktree).join("deckhand.json"),
                "{\"devCommand\":\"printf worktree-copy\"}",
            )
            .unwrap();
            assert_eq!(
                load_project_config(&worktree, None, None).await.unwrap().fingerprint,
                project.fingerprint
            );
            // An uncommitted edit in the main checkout applies to worktree sessions at once, and needs a fresh review.
            std::fs::write(
                Path::new(&cwd).join("deckhand.json"),
                "{\"devCommand\":\"printf live\"}",
            )
            .unwrap();
            let live = load_project_config(&worktree, None, None).await.unwrap();
            assert_eq!(live.config.dev_command_text(), Some("printf live"));
            assert!(project_needs_review(&live, config.trusted_projects.as_ref()));
            assert_eq!(dev(&live, &trusting(&live, &config)), "printf live");
            assert!(!exists(&Path::new(&cwd).join("DO_NOT_EXECUTE")));
        });
    }

    // covers: B-I6-07 trust keeps several fingerprints per repository, newest first and capped
    #[test]
    fn trust_keeps_several_fingerprints() {
        let (_dir, cwd) = repo();
        block_on(async {
            let first = load_project_config(&cwd, None, None).await.unwrap();
            let mut config = trusting(&first, &UserSettings::default());
            std::fs::write(Path::new(&cwd).join("deckhand.json"), "{}").unwrap();
            let second = load_project_config(&cwd, None, None).await.unwrap();
            config = trusting(&second, &trusting(&first, &config));
            assert!(trusted_by(&first, &config) && trusted_by(&second, &config));
            assert_eq!(
                config.trusted_projects.as_ref().unwrap().get(&first.trust_root),
                Some(&vec![second.fingerprint.clone(), first.fingerprint.clone()])
            );
            for index in 0..30 {
                let numbered = LoadedProject {
                    fingerprint: index.to_string(),
                    ..first.clone()
                };
                config = trusting(&numbered, &config);
            }
            assert_eq!(
                config
                    .trusted_projects
                    .as_ref()
                    .unwrap()
                    .get(&first.trust_root)
                    .map(Vec::len),
                Some(20)
            );
            assert!(!trusted_by(&first, &config));
        });
    }

    // covers: B-I6-08 trust roots are unique for submodules and bare repositories; bare repositories have no repository override
    #[test]
    fn trust_roots_for_submodules_and_bare_repositories() {
        let (_outer_dir, outer) = repo();
        let (_inner_dir, inner) = repo();
        tgit(
            &outer,
            &["-c", "protocol.file.allow=always", "submodule", "add", &inner, "sub"],
        );
        let sub = Path::new(&outer).join("sub").to_string_lossy().into_owned();
        let bare = Path::new(&outer).join("bare.git").to_string_lossy().into_owned();
        let linked = Path::new(&outer).join("bare-linked").to_string_lossy().into_owned();
        tgit(&outer, &["clone", "--bare", &inner, &bare]);
        tgit(&bare, &["worktree", "add", "-b", "linked", &linked]);
        block_on(async {
            let submodule = load_project_config(&sub, None, None).await.unwrap();
            assert_eq!(submodule.trust_root, real(Path::new(&sub)));
            assert_ne!(
                submodule.trust_root,
                load_project_config(&outer, None, None).await.unwrap().trust_root
            );
            let project = load_project_config(&linked, None, None).await.unwrap();
            assert_eq!(project.trust_root, real(Path::new(&bare)));
            assert_eq!(project.root, real(Path::new(&linked)));
            assert_eq!(project.path, None);
            assert!(!project.exists);
            let error = crate::config_document::read_project_config_document(&linked)
                .await
                .unwrap_err();
            assert!(error.to_string().contains("Bare repositories"), "{error}");
        });
    }

    // covers: B-I6-09 malformed, oversized and symlinked config errors are not hidden
    #[test]
    fn config_errors_are_not_hidden() {
        let (_dir, cwd) = repo();
        let file = Path::new(&cwd).join("deckhand.json");
        let load = || block_on(load_project_config(&cwd, None, None)).unwrap_err().to_string();
        std::fs::write(&file, "{").unwrap();
        assert!(load().contains("deckhand.json"));
        std::fs::write(&file, " ".repeat(70000)).unwrap();
        assert!(load().contains("64 KiB"));
        std::fs::remove_file(&file).unwrap();
        std::fs::write(Path::new(&cwd).join("target.json"), "{}").unwrap();
        std::os::unix::fs::symlink(Path::new(&cwd).join("target.json"), &file).unwrap();
        assert!(load().contains("must not be a symlink"));
    }
}
