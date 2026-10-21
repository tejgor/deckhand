//! Worktree links: file I/O (the pure parts are in deckhand_core::worktree_links).
//!
//! Applying `worktree.symlink` and `worktree.files` to a newly created worktree, and `userSlug` (which reads the OS
//! user). Templates, validation and merging are pure and live in `deckhand_core::worktree_links`; LinkResult is
//! deckhand-protocol's [`WorktreeLinks`].

use std::path::{Path, PathBuf};

use deckhand_core::error::{DhError, Result};
use deckhand_core::jsstr::path;
use deckhand_core::worktree_links::{expand_worktree_template, slugify_user, valid_relative_path};
use deckhand_protocol::data::{TemplateVars, WorktreeLinks, WorktreeSettings};

fn text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// `fs.realpath(file)` with Node's error message.
async fn realpath(file: &str) -> Result<String> {
    tokio::fs::canonicalize(file)
        .await
        .map(|resolved| text(&resolved))
        .map_err(|error| DhError::io(&error, "realpath", file))
}

/// TS: worktreeLinks.ts userSlug (the OS user name as a branch-safe slug, `user` when nothing is left; see
/// `deckhand_core::worktree_links::slugify_user`).
pub fn user_slug() -> String {
    let name = nix::unistd::User::from_uid(nix::unistd::getuid())
        .ok()
        .flatten()
        .map(|user| user.name)
        .unwrap_or_default();
    slugify_user(&name)
}

/// TS: worktreeLinks.ts inside (`target` is `root` or below it).
pub fn inside(root: &Path, target: &Path) -> bool {
    inside_str(&text(root), &text(target))
}

fn inside_str(root: &str, target: &str) -> bool {
    target == root
        || target
            .strip_prefix(root)
            .is_some_and(|rest| rest.starts_with(path::SEP))
}

/// TS: worktreeLinks.ts ensureParent (creates the parents of `relative` inside `root` one segment at a time, refusing
/// to pass through anything that resolves outside the worktree: `parent <dir> leaves the worktree`, `parent <dir> is
/// not a directory`). Returns the parent's real path.
pub async fn ensure_parent(root: &Path, relative: &str) -> Result<PathBuf> {
    let root = text(root);
    let mut current = root.clone();
    let parent = path::dirname(relative);
    for segment in parent.split('/').filter(|part| !part.is_empty() && *part != ".") {
        current = path::join(&[&current, segment]);
        let stat = match tokio::fs::symlink_metadata(&current).await {
            Ok(stat) => Some(stat),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(DhError::io(&error, "lstat", &current)),
        };
        match stat {
            None => tokio::fs::create_dir(&current)
                .await
                .map_err(|error| DhError::io(&error, "mkdir", &current))?,
            Some(stat) if stat.file_type().is_symlink() => {
                if !inside_str(&root, &realpath(&current).await?) {
                    return Err(DhError::msg(format!(
                        "parent {} leaves the worktree",
                        path::relative(&root, &current)
                    )));
                }
            }
            Some(stat) if !stat.is_dir() => {
                return Err(DhError::msg(format!(
                    "parent {} is not a directory",
                    path::relative(&root, &current)
                )));
            }
            Some(_) => {}
        }
    }
    let parent = realpath(&current).await?;
    if !inside_str(&root, &parent) {
        return Err(DhError::msg("parent leaves the worktree"));
    }
    Ok(PathBuf::from(parent))
}

/// TS: worktreeLinks.ts linkOne outcome.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkOutcome {
    Linked,
    /// Why the destination was skipped (`source missing, skipped`, `exists and is not a symlink, kept`, ...).
    Skipped(String),
}

fn skipped(why: impl Into<String>) -> Result<LinkOutcome> {
    Ok(LinkOutcome::Skipped(why.into()))
}

/// TS: worktreeLinks.ts linkOne (existing real content is never replaced; an existing symlink is replaced
/// atomically).
pub async fn link_one(root: &Path, destination: &str, source: &Path) -> Result<LinkOutcome> {
    if !valid_relative_path(destination) {
        return skipped("invalid destination");
    }
    let source = text(source);
    let target = match tokio::fs::canonicalize(&source).await {
        Ok(target) => text(&target),
        Err(error) => {
            let error = DhError::io(&error, "realpath", &source);
            return skipped(if error.is_not_found() {
                String::from("source missing, skipped")
            } else {
                format!("source unavailable: {error}")
            });
        }
    };
    let parent = text(&ensure_parent(root, destination).await?);
    let file = path::join(&[&parent, &path::basename(destination)]);
    let existing = tokio::fs::symlink_metadata(&file).await.ok();
    if existing.is_some_and(|stat| !stat.file_type().is_symlink()) {
        return skipped("exists and is not a symlink, kept");
    }
    if inside_str(&file, &target) {
        return skipped("source is the destination itself, skipped");
    }
    let id = uuid::Uuid::new_v4().to_string();
    let temporary = path::join(&[&parent, &format!(".deckhand-link-{}", id.get(..8).unwrap_or(&id))]);
    tokio::fs::symlink(&target, &temporary)
        .await
        .map_err(|error| DhError::io_dest(&error, "symlink", &target, &temporary))?;
    if let Err(error) = tokio::fs::rename(&temporary, &file).await {
        let _ = tokio::fs::remove_file(&temporary).await;
        return Err(DhError::io_dest(&error, "rename", &temporary, &file));
    }
    Ok(LinkOutcome::Linked)
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
    let mut result = WorktreeLinks::default();
    let Some(settings) = settings else {
        return result;
    };
    let root = match realpath(&options.worktree_root).await {
        Ok(root) => root,
        Err(error) => {
            result.notes.push(format!("worktree unavailable: {error}"));
            return result;
        }
    };
    let launch = options.launch_root.as_str();
    let mut entries: Vec<(String, String)> = settings
        .symlink
        .iter()
        .flatten()
        .map(|entry| (entry.clone(), path::join(&[launch, entry])))
        .collect();
    for (destination, source) in settings.files.iter().flatten() {
        // An invalid template becomes an empty source: noted below, never thrown.
        let resolved = expand_worktree_template(source, &options.vars).unwrap_or_default();
        let source = if resolved.is_empty() {
            resolved
        } else {
            path::resolve_all(&[launch, &resolved])
        };
        entries.push((destination.clone(), source));
    }
    let root = PathBuf::from(root);
    for (destination, source) in entries {
        let outcome = if source.is_empty() {
            Ok(LinkOutcome::Skipped(String::from("invalid source")))
        } else {
            link_one(&root, &destination, Path::new(&source)).await
        };
        match outcome {
            Ok(LinkOutcome::Linked) => result.linked.push(destination),
            Ok(LinkOutcome::Skipped(why)) => result.notes.push(format!("{destination}: {why}")),
            Err(error) => result.notes.push(format!("{destination}: {error}")),
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::tests::block_on;
    use deckhand_testkit::{DirOp, Outcome, Replay, dir_cases};

    fn apply_op(op: &DirOp) -> Outcome {
        let settings: Option<WorktreeSettings> = if op.arg_is_undefined(0) {
            None
        } else {
            match op.arg_as(0) {
                Ok(settings) => Some(settings),
                Err(error) => return Outcome::throws(format!("parity: settings: {error}")),
            }
        };
        let options = op.arg(1);
        let field = |key: &str| {
            options
                .get(key)
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .to_owned()
        };
        let vars: TemplateVars = match options.get("vars").cloned().map(serde_json::from_value) {
            Some(Ok(vars)) => vars,
            _ => return Outcome::throws("parity: vars"),
        };
        let options = LinkOptions {
            launch_root: field("launchRoot"),
            worktree_root: field("worktreeRoot"),
            vars,
        };
        Outcome::ret(block_on(apply_worktree_links(settings.as_ref(), &options)))
    }

    #[test]
    fn apply_worktree_links_parity() {
        let cases = dir_cases("worktreeLinks-I6");
        assert!(!cases.is_empty());
        let mut failed = Vec::new();
        for fixture in cases {
            let report = Replay::new(&fixture).run_dir(|_root, op| match op.fn_name.as_str() {
                "applyWorktreeLinks" => apply_op(op),
                other => Outcome::throws(format!("parity: unknown op {other}")),
            });
            if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| report.assert_ok())).is_err() {
                failed.push(report.summary());
            }
        }
        assert!(failed.is_empty(), "{}", failed.join("\n\n"));
    }

    #[test]
    fn inside_matches_whole_segments() {
        assert!(inside(Path::new("/a/b"), Path::new("/a/b")));
        assert!(inside(Path::new("/a/b"), Path::new("/a/b/c")));
        assert!(!inside(Path::new("/a/b"), Path::new("/a/bc")));
        assert!(!inside(Path::new("/a/b"), Path::new("/a")));
    }

    fn vars(name: &str, repo: &str, repo_parent: &str, repo_root: &str, home: &str) -> TemplateVars {
        TemplateVars {
            name: name.into(),
            repo: repo.into(),
            repo_parent: repo_parent.into(),
            repo_root: repo_root.into(),
            home: home.into(),
        }
    }

    #[test]
    fn templates_validation_merge_and_trust() {
        use deckhand_core::project_config::{resolve_settings, trust_project_config, validate_project_config};
        use deckhand_core::worktree_links::worktree_location;
        use deckhand_protocol::data::LoadedProject;
        use deckhand_protocol::records::AppConfig;
        use serde_json::{Value, json};

        let vars = vars("feat/x", "app", "/dev", "/dev/app", "/h");
        assert_eq!(
            worktree_location("{repoParent}/worktrees/{name}", &vars).unwrap(),
            "/dev/worktrees/feat/x"
        );
        assert_eq!(
            worktree_location("~/wt/{repo}/{name}", &vars).unwrap(),
            "/h/wt/app/feat/x"
        );
        assert_eq!(
            expand_worktree_template("{repoRoot}-{repo}", &vars).unwrap(),
            "/dev/app-app"
        );
        let error = expand_worktree_template("/x/{nope}", &vars).unwrap_err();
        assert!(error.to_string().contains("Unknown placeholder"), "{error}");
        let error = worktree_location("wt/{name}", &vars).unwrap_err();
        assert!(error.to_string().contains("absolute"), "{error}");
        let many: Vec<String> = (0..51).map(|index| format!("d{index}")).collect();
        let invalid = [
            json!({"location": "relative/{name}"}),
            json!({"location": "/x/{nope}/{name}"}),
            json!({"location": "/x/{name"}),
            json!({"location": "/fixed"}),
            json!({"symlink": ["../up"]}),
            json!({"symlink": ["/abs"]}),
            json!({"symlink": ["a/../b"]}),
            json!({"symlink": ["./a"]}),
            json!({"symlink": ["a/"]}),
            json!({"symlink": [".git/hooks"]}),
            json!({"symlink": "a"}),
            json!({"symlink": many}),
            json!({"files": {"a\u{0}b": "/x"}}),
            json!({"files": {"../x": "/x"}}),
            json!({"files": {"ok": ""}}),
            json!({"other": true}),
        ];
        for worktree in invalid {
            assert!(
                validate_project_config(&json!({"worktree": worktree}), None).is_err(),
                "{worktree}"
            );
        }
        let global = json!({"location": "~/global/{name}", "symlink": ["a", "b"],
            "files": {"x/.env": "{repoParent}/global.env", "z": "/global-z"}});
        let project: LoadedProject = serde_json::from_value(json!({"root": "/r", "trustRoot": "/r",
            "path": "/r/deckhand.json", "exists": true, "fingerprint": "f",
            "config": {"worktree": {"location": "{repoParent}/repo/{name}", "symlink": ["b", "c"],
                "files": {"x/.env": "repo.env", "y": "/repo-y"}}}}))
        .unwrap();
        let user: AppConfig = serde_json::from_value(json!({"defaults": {"worktree": global.clone()}})).unwrap();
        let worktree_of = |user: &AppConfig, project: Option<&LoadedProject>| -> Value {
            let settings = resolve_settings(project, user, "/config").unwrap();
            serde_json::to_value(&settings.worktree).unwrap()
        };
        // Untrusted: global only.
        assert_eq!(worktree_of(&user, Some(&project)), global);
        let trusted = AppConfig {
            trusted_projects: Some(trust_project_config(&project.trust_root, &project.fingerprint, None)),
            ..user.clone()
        };
        assert_eq!(
            worktree_of(&trusted, Some(&project)),
            json!({"location": "{repoParent}/repo/{name}", "symlink": ["a", "b", "c"],
                "files": {"x/.env": "repo.env", "z": "/global-z", "y": "/repo-y"}})
        );
        assert_eq!(worktree_of(&AppConfig::default(), None), Value::Null);
    }

    #[test]
    fn links_resolve_real_targets_and_refuse_escapes() {
        use crate::config::tests::{exists, real, repo, tgit};
        use crate::git::remove_worktree;
        use crate::storage::tests::Home;

        let home = tempfile::tempdir().unwrap();
        let _home = Home::set(home.path());
        let (_launch_dir, launch) = repo();
        let outside_dir = tempfile::Builder::new().prefix("dh-links-").tempdir().unwrap();
        let outside = real(outside_dir.path());
        let at = |base: &str, relative: &str| Path::new(base).join(relative).to_string_lossy().into_owned();
        std::fs::create_dir(at(&outside, "deps")).unwrap();
        std::fs::write(at(&outside, "deps/keep.js"), "dependency").unwrap();
        std::fs::write(at(&outside, "backend.env"), "SECRET=1").unwrap();
        // The launch checkout's node_modules is itself a link: the new one points at the real directory.
        std::fs::create_dir(at(&launch, "frontend")).unwrap();
        std::os::unix::fs::symlink(at(&outside, "deps"), at(&launch, "frontend/node_modules")).unwrap();
        std::fs::write(at(&launch, "tracked.txt"), "real").unwrap();
        std::os::unix::fs::symlink(&outside, at(&launch, "escape")).unwrap();
        tgit(&launch, &["add", "tracked.txt", "escape"]);
        tgit(&launch, &["commit", "-m", "tracked"]);
        let worktree = at(&outside, "wt");
        tgit(&launch, &["worktree", "add", "-b", "wt", &worktree]);
        std::fs::create_dir(at(&worktree, "backend")).unwrap();
        // A stale link is replaced.
        std::os::unix::fs::symlink("/nonexistent", at(&worktree, "backend/.env")).unwrap();
        let settings: WorktreeSettings = serde_json::from_value(serde_json::json!({
            "symlink": ["frontend/node_modules", "backend/.venv", "tracked.txt"],
            "files": {"backend/.env": "{repoParent}/backend.env", "escape/stolen": "{repoParent}/backend.env"}
        }))
        .unwrap();
        let options = LinkOptions {
            launch_root: launch.clone(),
            worktree_root: worktree.clone(),
            vars: vars("wt", "app", &outside, &launch, &outside),
        };
        block_on(async {
            let result = apply_worktree_links(Some(&settings), &options).await;
            assert_eq!(result.linked, ["frontend/node_modules", "backend/.env"]);
            assert_eq!(
                std::fs::read_link(at(&worktree, "frontend/node_modules")).unwrap(),
                Path::new(&at(&outside, "deps"))
            );
            assert_eq!(
                std::fs::read_to_string(at(&worktree, "backend/.env")).unwrap(),
                "SECRET=1"
            );
            let notes = result.notes.join("\n");
            assert!(notes.contains("backend/.venv: source missing"), "{notes}");
            assert!(
                notes.contains("tracked.txt: exists and is not a symlink, kept"),
                "{notes}"
            );
            assert!(
                notes.contains("escape/stolen: parent escape leaves the worktree"),
                "{notes}"
            );
            assert_eq!(std::fs::read_to_string(at(&worktree, "tracked.txt")).unwrap(), "real");
            assert!(!exists(Path::new(&at(&outside, "stolen"))));
            // inspectWorkspaceCleanup(worktree, 'main', {deleteBranch: true}).safe (workspaceGit.ts) is unit I4's to
            // port; see the I6 manifest notes.
            remove_worktree(&worktree, &launch, Some("wt")).await.unwrap();
        });
        assert!(!exists(Path::new(&worktree)));
        assert_eq!(
            std::fs::read_to_string(at(&outside, "deps/keep.js")).unwrap(),
            "dependency"
        );
        assert_eq!(
            std::fs::read_to_string(at(&outside, "backend.env")).unwrap(),
            "SECRET=1"
        );
    }

    /// `git(...).catch(() => '')`: trimmed stdout, or empty when git fails.
    fn git_or_empty(dir: &str, args: &[&str]) -> String {
        std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .ok()
            .filter(|output| output.status.success())
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
            .unwrap_or_default()
    }

    #[test]
    fn branch_from_and_branch_name_decide_the_new_branch() {
        use crate::config::tests::{exists, real, repo, tgit};
        use crate::git::create_worktree_for_session;
        use crate::storage::tests::Home;
        use deckhand_core::project_config::validate_project_config;
        use deckhand_protocol::data::{BranchFrom, WorktreeOrigin};
        use serde_json::json;

        for worktree in [
            json!({"branchFrom": "upstream"}),
            json!({"branchName": "feat/x"}),
            json!({"branchName": "bad name/{name}"}),
            json!({"branchName": "{name}..x"}),
            json!({"branchName": "{name}.lock"}),
            json!({"branchName": "{nope}/{name}"}),
            json!({"hook": "no"}),
        ] {
            assert!(
                validate_project_config(&json!({"worktree": worktree}), None).is_err(),
                "{worktree}"
            );
        }
        let valid = json!({"branchFrom": "default", "branchName": "feat/{user}-{name}", "hook": false});
        let validated = validate_project_config(&json!({"worktree": valid}), None).unwrap();
        assert_eq!(serde_json::to_value(&validated.worktree).unwrap(), valid);

        let home_dir = tempfile::Builder::new().prefix("deckhand-branch-").tempdir().unwrap();
        let _home = Home::set(home_dir.path());
        let home = real(home_dir.path());
        let (_dir, cwd) = repo();
        let at = |base: &str, relative: &str| Path::new(base).join(relative).to_string_lossy().into_owned();
        let (remote, other) = (at(&home, "origin.git"), at(&home, "other"));
        tgit(&home, &["clone", "--bare", &cwd, &remote]);
        tgit(&cwd, &["remote", "add", "origin", &remote]);
        tgit(&cwd, &["fetch", "origin"]);
        tgit(&cwd, &["remote", "set-head", "origin", "main"]);
        // origin/main gets a commit the local main does not have; the launch checkout is on a feature branch.
        tgit(&home, &["clone", &remote, &other]);
        tgit(&other, &["config", "user.email", "o@example.invalid"]);
        tgit(&other, &["config", "user.name", "O"]);
        std::fs::write(at(&other, "file.txt"), "remote\n").unwrap();
        tgit(&other, &["commit", "-am", "remote change"]);
        tgit(&other, &["push", "origin", "main"]);
        let remote_head = tgit(&other, &["rev-parse", "HEAD"]);
        let local_main = tgit(&cwd, &["rev-parse", "main"]);
        tgit(&cwd, &["checkout", "-b", "feature"]);
        std::fs::write(at(&cwd, "file.txt"), "feature\n").unwrap();
        tgit(&cwd, &["commit", "-am", "feature"]);
        let feature = tgit(&cwd, &["rev-parse", "HEAD"]);

        let settings = |branch_from: Option<BranchFrom>, branch_name: Option<&str>| WorktreeSettings {
            branch_from,
            branch_name: branch_name.map(str::to_owned),
            ..WorktreeSettings::default()
        };
        block_on(async {
            let current = create_worktree_for_session("from current", &cwd, None, None, None)
                .await
                .unwrap();
            assert_eq!(current.branch, "from_current");
            assert_eq!(current.head, feature);
            assert_eq!(current.base_ref.as_deref(), Some("feature"));
            let from_default = settings(Some(BranchFrom::Default), Some("{user}/{name}"));
            let local = create_worktree_for_session("from default", &cwd, None, Some(&from_default), None)
                .await
                .unwrap();
            assert_eq!(local.branch, format!("{}/from_default", user_slug()));
            assert_eq!(local.head, local_main);
            assert_eq!(local.base_ref.as_deref(), Some("main"));
            let from_origin = settings(Some(BranchFrom::Origin), Some("feat/{name}"));
            let fetched = create_worktree_for_session("from origin", &cwd, None, Some(&from_origin), None)
                .await
                .unwrap();
            assert_eq!(fetched.branch, "feat/from_origin");
            assert_eq!(fetched.head, remote_head);
            assert_eq!(fetched.base_ref.as_deref(), Some("origin/main"));
            // --no-track
            assert_eq!(
                git_or_empty(&cwd, &["config", "--get", "branch.feat/from_origin.remote"]),
                ""
            );
            // Reuse: an existing worktree path is reused as before, without a base.
            let reused = create_worktree_for_session("from origin", &cwd, None, Some(&from_origin), None)
                .await
                .unwrap();
            assert_eq!(reused.origin, WorktreeOrigin::Existing);
            assert_eq!(reused.base_ref, None);
            let bad = settings(None, Some("{name}..x"));
            let error = create_worktree_for_session("bad", &cwd, None, Some(&bad), None)
                .await
                .unwrap_err();
            assert!(error.to_string().contains("Invalid branch name"), "{error}");
            tgit(&cwd, &["remote", "set-url", "origin", &at(&home, "missing.git")]);
            let offline = settings(Some(BranchFrom::Origin), None);
            let error = create_worktree_for_session("offline", &cwd, None, Some(&offline), None)
                .await
                .unwrap_err();
            assert!(error.to_string().contains("git fetch origin main failed"), "{error}");
        });
        assert!(!exists(Path::new(&at(&home, "worktrees/offline"))));
        assert_eq!(tgit(&cwd, &["branch", "--list", "offline"]), "");
    }

    #[test]
    fn user_slug_is_branch_safe() {
        let slug = user_slug();
        assert!(!slug.is_empty());
        assert!(
            slug.chars()
                .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || matches!(ch, '.' | '_' | '-'))
        );
    }
}
