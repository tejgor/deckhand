//! Project config files: reading and writing them (the pure parts are in deckhand_core::project_config).
//!
//! Reading the repository override (the main checkout's live deckhand.json; copies inside linked worktrees are
//! ignored) and the trust decision for a deckhand.json saved through Deckhand. Validation, fingerprints and effective settings are pure and live in
//! `deckhand_core::project_config`.

use std::io::Read;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;

use deckhand_core::config_draft::MAX_CONFIG_BYTES;
use deckhand_core::error::{DhError, Result, errno_code, errno_description};
use deckhand_core::jsstr::path;
use deckhand_core::project_config::{
    PROJECT_CONFIG_FILE, UserSettings, is_project_trusted, parse_project_config, project_fingerprint,
};
use deckhand_protocol::Ordered;
use deckhand_protocol::data::{LoadedProject, ProjectConfig, SaveTrust};

use crate::git::{RepoContext, resolve_repo_context};

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

#[cfg(test)]
fn bounded(max: usize) -> ReadBounded {
    ReadBounded { no_follow: false, max }
}

/// TS: projectConfig.ts loadProjectConfig: the main checkout's deckhand.json (errors prefixed `<file>: `).
/// `context`: the repository context when the caller has it (else resolved from `cwd`).
pub async fn load_project_config(cwd: &str, context: Option<&RepoContext>) -> Result<LoadedProject> {
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
    let fingerprint = project_fingerprint(&raw);
    Ok(LoadedProject {
        root,
        trust_root,
        path: file,
        config,
        exists,
        fingerprint,
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

/// TS: projectConfig.ts savedProjectTrust: a self-edit keeps trust only when the bytes it replaced (exactly what the
/// user edited from) were trusted; a new file is trusted, since there was nothing to review.
pub async fn saved_project_trust(
    context: &RepoContext,
    previous: &PreviousDocument,
    next: &str,
    user: &UserSettings,
) -> Result<SavedTrust> {
    let unreviewed = SavedTrust {
        trust: SaveTrust::Unreviewed,
        fingerprint: None,
    };
    // Never loadable, so never trusted.
    if previous.exists && parse_project_config(&previous.raw, None).is_err() {
        return Ok(unreviewed);
    }
    parse_project_config(next, None)?;
    if previous.exists
        && !is_project_trusted(
            &context.trust_root,
            &project_fingerprint(&previous.raw),
            user.trusted_projects.as_ref(),
        )
    {
        return Ok(unreviewed);
    }
    Ok(SavedTrust {
        trust: if previous.exists {
            SaveTrust::Kept
        } else {
            SaveTrust::Created
        },
        fingerprint: Some(project_fingerprint(next)),
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::git::create_worktree_for_session;
    use crate::storage::tests::{Home, block_on};
    use deckhand_core::project_config::{project_needs_review, trust_project_config};
    use deckhand_protocol::data::WorktreeCreator;
    use deckhand_testkit::{DirOp, Outcome, Replay, dir_cases, tags};
    use serde_json::{Value, json};
    use std::process::Command;

    /// Replays every directory case of `module`; fails with every failing case's summary.
    pub(crate) fn replay_dir_cases(module: &str, mut port: impl FnMut(&Path, &DirOp) -> Outcome) {
        let cases = dir_cases(module);
        assert!(!cases.is_empty(), "no directory cases for {module}");
        let mut failed = Vec::new();
        for fixture in cases {
            let report = Replay::new(&fixture).run_dir(&mut port);
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
                // The recorded user config (argument 1) only switched a creation hook on or off.
                let context = context_arg(op.arg(2));
                block_on(load_project_config(op.arg_str(0)?, Some(&context)))
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

    #[test]
    fn a_leftover_creation_script_is_neither_reviewed_nor_run() {
        let (_dir, cwd) = repo();
        let home = tempfile::Builder::new().prefix("deckhand-old-hook-").tempdir().unwrap();
        let _home = Home::set(home.path());
        block_on(async {
            let bare = load_project_config(&cwd, None).await.unwrap();
            let file = Path::new(&cwd).join(".claude/scripts/create-worktree.sh");
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(&file, "#!/bin/bash\ntouch \"$CLAUDE_PROJECT_DIR/hook-ran\"\nexit 1\n").unwrap();
            let project = load_project_config(&cwd, None).await.unwrap();
            assert_eq!(project.fingerprint, bare.fingerprint);
            assert!(!project_needs_review(&project, None));
            let created = create_worktree_for_session("no hook", &cwd, None, None).await.unwrap();
            assert_eq!(created.creator, WorktreeCreator::Fallback);
            assert!(created.path.starts_with(&real(home.path())), "{}", created.path);
            assert!(!exists(&Path::new(&cwd).join("hook-ran")));
            // A file that still switches it on or off reads as before; the setting does nothing.
            for raw in ["{\"worktree\":{\"hook\":false}}", "{\"worktree\":{\"hook\":true}}"] {
                std::fs::write(Path::new(&cwd).join("deckhand.json"), raw).unwrap();
                let project = load_project_config(&cwd, None).await.unwrap();
                assert!(project.exists && project_needs_review(&project, None));
            }
        });
    }

    fn user(value: Value) -> UserSettings {
        serde_json::from_value(value).unwrap()
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
                &load_project_config(&other, None).await.unwrap(),
                None
            ));
            let raw = "{\"devCommand\":\"touch DO_NOT_EXECUTE\",\"defaultWorkspace\":\"new\"}";
            std::fs::write(Path::new(&cwd).join("deckhand.json"), raw).unwrap();
            std::fs::write(Path::new(&other).join("deckhand.json"), raw).unwrap();
            tgit(&cwd, &["add", "deckhand.json"]);
            tgit(&cwd, &["commit", "-m", "project config"]);
            let worktree = Path::new(&cwd).join("linked").to_string_lossy().into_owned();
            tgit(&cwd, &["worktree", "add", "-b", "linked", &worktree]);
            let project = load_project_config(&worktree, None).await.unwrap();
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
            assert!(trusted_by(&load_project_config(&cwd, None).await.unwrap(), &config));
            assert!(!trusted_by(&load_project_config(&other, None).await.unwrap(), &config));
            std::fs::write(
                Path::new(&worktree).join("deckhand.json"),
                "{\"devCommand\":\"printf worktree-copy\"}",
            )
            .unwrap();
            assert_eq!(
                load_project_config(&worktree, None).await.unwrap().fingerprint,
                project.fingerprint
            );
            // An uncommitted edit in the main checkout applies to worktree sessions at once, and needs a fresh review.
            std::fs::write(
                Path::new(&cwd).join("deckhand.json"),
                "{\"devCommand\":\"printf live\"}",
            )
            .unwrap();
            let live = load_project_config(&worktree, None).await.unwrap();
            assert_eq!(live.config.dev_command_text(), Some("printf live"));
            assert!(project_needs_review(&live, config.trusted_projects.as_ref()));
            assert_eq!(dev(&live, &trusting(&live, &config)), "printf live");
            assert!(!exists(&Path::new(&cwd).join("DO_NOT_EXECUTE")));
        });
    }

    #[test]
    fn trust_keeps_several_fingerprints() {
        let (_dir, cwd) = repo();
        block_on(async {
            let first = load_project_config(&cwd, None).await.unwrap();
            let mut config = trusting(&first, &UserSettings::default());
            std::fs::write(Path::new(&cwd).join("deckhand.json"), "{}").unwrap();
            let second = load_project_config(&cwd, None).await.unwrap();
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
            let submodule = load_project_config(&sub, None).await.unwrap();
            assert_eq!(submodule.trust_root, real(Path::new(&sub)));
            assert_ne!(
                submodule.trust_root,
                load_project_config(&outer, None).await.unwrap().trust_root
            );
            let project = load_project_config(&linked, None).await.unwrap();
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

    #[test]
    fn config_errors_are_not_hidden() {
        let (_dir, cwd) = repo();
        let file = Path::new(&cwd).join("deckhand.json");
        let load = || block_on(load_project_config(&cwd, None)).unwrap_err().to_string();
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
