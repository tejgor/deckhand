//! The daemon's project config, settings and trust.
//!
//! The Settings view's requests: the repository's deckhand.json and the global defaults (read, saved revision-checked,
//! never running anything), trusting a reviewed repository configuration, the worktree link candidates, and the branch
//! list the new-session flow offers.

use deckhand_core::config_draft::MAX_CONFIG_LABEL;
use deckhand_core::error::{DhError, Result};
use deckhand_core::jsstr::locale_compare;
use deckhand_core::project_config::{is_project_trusted, project_needs_review, resolve_settings, trust_project_config};
use deckhand_core::project_config_document::check_draft;
use deckhand_protocol::data::{
    BranchFrom, BranchList, CandidateSizes, ConfigTargetKind, LoadedProject, ProjectInfo, SavedConfigDocument,
    SettingsInfo, WorktreeCandidates,
};
use deckhand_protocol::records::AppConfig;
use deckhand_protocol::wire::{CwdRequest, Loose, SaveConfig, TrustProject, WorktreeCandidateSizes};
use deckhand_sys::config::load_project_config;
use deckhand_sys::config_document::{SaveDocumentOptions, save_global_defaults_document, save_project_config_document};
use deckhand_sys::git::{BranchWhere, current_branch, find_repo_root, list_local_branches, resolve_default_branch};
use deckhand_sys::paths;
use deckhand_sys::settings_info::{
    MAX_SIZE_PATHS, read_candidate_sizes, read_global_link_candidates, read_settings_info, read_worktree_candidates,
};
use deckhand_sys::storage::{load_app_config, update_app_config};
use serde_json::Value;

use crate::daemon::Daemon;
use crate::dispatch::{as_given, path_argument};

/// The configuration's part of the daemon state (none: configuration is read from disk for every request).
#[derive(Debug, Default)]
pub struct ConfigState {}

/// paths.ts getConfigDir, as the effective settings use it (messages, the built-in worktree location).
fn config_dir() -> String {
    paths::config_dir().to_string_lossy().into_owned()
}

/// TS: daemon.ts projectInfo: `{...project, trusted, needsReview, effective}` (invalid global defaults fail here).
pub fn project_info(project: LoadedProject, config: &AppConfig) -> Result<ProjectInfo> {
    let trusted_projects = config.trusted_projects.as_ref();
    let trusted = is_project_trusted(&project.trust_root, &project.fingerprint, trusted_projects);
    let needs_review = project_needs_review(&project, trusted_projects);
    // Effective settings may throw for invalid global defaults; that surfaces wherever they are used.
    let effective = resolve_settings(Some(&project), config, &config_dir())?;
    Ok(ProjectInfo {
        project,
        trusted,
        needs_review,
        effective,
    })
}

impl Daemon {
    /// TS: handleRequest `save-config`: the global defaults, or the repository's deckhand.json (kept trusted when the
    /// version it replaces was).
    pub async fn handle_save_config(&self, request: &SaveConfig) -> Result<SavedConfigDocument> {
        // Saving never runs anything. A repository file saved here stays trusted when the version it replaces was
        // trusted (or absent), never newly trusting a creation hook; see saved_project_trust.
        // projectConfigDocument.ts checkDraft on what was sent: text first, then a revision that is null or a string.
        let Some(raw) = request.raw.as_deref() else {
            return Err(DhError::msg(format!(
                "Configuration must be text of at most {MAX_CONFIG_LABEL}"
            )));
        };
        check_draft(raw, None)?;
        let revision = match &request.revision {
            Loose::Known(revision) => Some(revision.as_str()),
            Loose::Other(Value::Null) => None,
            _ => return Err(DhError::msg("Invalid configuration revision")),
        };
        if request.target == ConfigTargetKind::Global {
            let document = save_global_defaults_document(raw, revision).await?;
            return Ok(SavedConfigDocument { document, trust: None });
        }
        save_project_config_document(
            &as_given(&request.cwd),
            raw,
            revision,
            SaveDocumentOptions { keep_trust: true },
        )
        .await
    }

    /// TS: handleRequest `settings-info` (settingsInfo.ts readSettingsInfo).
    pub async fn handle_settings_info(&self, request: &CwdRequest) -> Result<SettingsInfo> {
        // Node's path API refuses a non-string before anything else answers (readConfigTargets).
        read_settings_info(path_argument(&request.cwd, "path")?).await
    }

    /// TS: handleRequest `worktree-candidates` (settingsInfo.ts readWorktreeCandidates).
    pub async fn handle_worktree_candidates(&self, request: &CwdRequest) -> Result<WorktreeCandidates> {
        read_worktree_candidates(&as_given(&request.cwd)).await
    }

    /// TS: handleRequest `global-link-candidates`: candidates across every session's repository, most recently
    /// updated first.
    pub async fn handle_global_link_candidates(&self, request: &CwdRequest) -> Result<WorktreeCandidates> {
        let roots = self.link_candidate_roots();
        read_global_link_candidates(&as_given(&request.cwd), &roots).await
    }

    /// The repositories the user works in: every session's root, most recently updated first (a stable sort, as
    /// `Array.prototype.sort` with `localeCompare`).
    fn link_candidate_roots(&self) -> Vec<String> {
        let state = self.sessions.borrow();
        let mut sessions: Vec<(&str, &str)> = state
            .sessions
            .values()
            .map(|session| (session.updated_at.as_str(), session.repo_root.as_str()))
            .collect();
        sessions.sort_by(|a, b| locale_compare(b.0, a.0));
        sessions.into_iter().map(|(_, root)| root.to_owned()).collect()
    }

    /// TS: handleRequest `worktree-candidate-sizes` (settingsInfo.ts readCandidateSizes).
    pub async fn handle_worktree_candidate_sizes(&self, request: &WorktreeCandidateSizes) -> Result<CandidateSizes> {
        // Anything but an array of strings fails the paths check, as in TS (before the repository is looked up).
        match &request.paths {
            Some(paths) => read_candidate_sizes(&as_given(&request.cwd), paths).await,
            None => Err(DhError::msg(format!(
                "Pass at most {MAX_SIZE_PATHS} relative candidate paths"
            ))),
        }
    }

    /// TS: handleRequest `project-info`.
    pub async fn handle_project_info(&self, request: &CwdRequest) -> Result<ProjectInfo> {
        let config = load_app_config().await?;
        let project = load_project_config(&as_given(&request.cwd), Some(&config), None).await?;
        project_info(project, &config)
    }

    /// TS: handleRequest `trust-project` (`Repository configuration changed; review it again before trusting`).
    pub async fn handle_trust_project(&self, request: &TrustProject) -> Result<ProjectInfo> {
        let user = load_app_config().await?;
        let project = load_project_config(&as_given(&request.cwd), Some(&user), None).await?;
        if request.fingerprint.known() != Some(&project.fingerprint) {
            return Err(DhError::msg(
                "Repository configuration changed; review it again before trusting",
            ));
        }
        let (trust_root, fingerprint) = (project.trust_root.clone(), project.fingerprint.clone());
        let config = update_app_config(move |current| {
            let trusted_projects = trust_project_config(&trust_root, &fingerprint, current.trusted_projects.as_ref());
            Ok(AppConfig {
                trusted_projects: Some(trusted_projects),
                ..current
            })
        })
        .await?;
        project_info(project, &config)
    }

    /// TS: InkDaemon.listBranches: local branches, current and default branch, the configured branchFrom and whether
    /// a trusted creation hook will create the worktree.
    pub async fn list_branches(&self, cwd: Option<&str>) -> Result<BranchList> {
        let root = find_repo_root(cwd).await?;
        let app_config = load_app_config().await?;
        let project = load_project_config(&root, Some(&app_config), None).await.ok();
        let branch_from = resolve_settings(project.as_ref(), &app_config, &config_dir())
            .ok()
            .and_then(|settings| settings.worktree.as_ref().and_then(|worktree| worktree.branch_from))
            .unwrap_or(BranchFrom::Current);
        let (branches, current, default_branch) = tokio::join!(
            list_local_branches(&root, None),
            current_branch(&root),
            resolve_default_branch(&root, None, BranchWhere::Local),
        );
        let current = current.unwrap_or_default();
        let hook = project.as_ref().is_some_and(|project| {
            project.creation_hook.is_some()
                && is_project_trusted(
                    &project.trust_root,
                    &project.fingerprint,
                    app_config.trusted_projects.as_ref(),
                )
        });
        Ok(BranchList {
            branches,
            current: (!current.is_empty()).then_some(current),
            default_branch: default_branch.filter(|name| !name.is_empty()),
            branch_from,
            hook: hook.then_some(true),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::process::Command;
    use std::sync::MutexGuard;

    use deckhand_protocol::Ordered;
    use deckhand_protocol::data::SessionRecord;
    use serde_json::{Value, json};

    use super::*;
    use crate::daemon::{DaemonOptions, DaemonPaths};

    /// A scratch DECKHAND_HOME (and the daemon's paths in it) while this lives.
    struct Home {
        _guard: MutexGuard<'static, ()>,
        dir: tempfile::TempDir,
        previous: Option<std::ffi::OsString>,
    }

    impl Home {
        fn new() -> Home {
            let guard = crate::server::tests::env_lock();
            let dir = tempfile::Builder::new().prefix("deckhand-home-").tempdir().unwrap();
            let previous = std::env::var_os(paths::HOME_ENV);
            // SAFETY: tests touching the environment hold server::tests::env_lock.
            unsafe { std::env::set_var(paths::HOME_ENV, dir.path()) };
            Home {
                _guard: guard,
                dir,
                previous,
            }
        }

        fn daemon(&self) -> Daemon {
            let dir = self.dir.path();
            Daemon::with_paths(
                DaemonOptions::default(),
                DaemonPaths {
                    config_dir: dir.to_path_buf(),
                    socket: dir.join("daemon.sock"),
                    pid_file: dir.join("daemon.pid"),
                    log_file: dir.join("daemon.log"),
                    worker_dir: dir.join("workers"),
                },
            )
        }

        fn config(&self) -> Value {
            let text = std::fs::read_to_string(self.dir.path().join("config.json")).unwrap_or_default();
            serde_json::from_str(&text).unwrap_or(Value::Null)
        }
    }

    impl Drop for Home {
        fn drop(&mut self) {
            // SAFETY: as in Home::new (the environment lock is still held).
            unsafe {
                match self.previous.take() {
                    Some(value) => std::env::set_var(paths::HOME_ENV, value),
                    None => std::env::remove_var(paths::HOME_ENV),
                }
            }
        }
    }

    fn block_on<F: std::future::Future>(future: F) -> F::Output {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        tokio::task::LocalSet::new().block_on(&runtime, future)
    }

    fn tgit(cwd: &str, args: &[&str]) -> String {
        let output = Command::new("git")
            .args(args)
            .current_dir(cwd)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    }

    /// tests/helpers.ts `repo()`: a repository on `main` with one commit (file.txt).
    fn repo() -> (tempfile::TempDir, String) {
        let dir = tempfile::Builder::new().prefix("deckhand-test-").tempdir().unwrap();
        let path = std::fs::canonicalize(dir.path())
            .unwrap()
            .to_string_lossy()
            .into_owned();
        tgit(&path, &["init", "-b", "main"]);
        tgit(&path, &["config", "user.name", "Deckhand Test"]);
        tgit(&path, &["config", "user.email", "deckhand@example.invalid"]);
        tgit(&path, &["config", "commit.gpgsign", "false"]);
        std::fs::write(Path::new(&path).join("file.txt"), "first\n").unwrap();
        tgit(&path, &["add", "."]);
        tgit(&path, &["commit", "-m", "initial"]);
        (dir, path)
    }

    fn cwd_request(cwd: &str) -> CwdRequest {
        CwdRequest {
            request_id: "r".to_owned(),
            cwd: cwd.into(),
        }
    }

    fn trust_request(cwd: &str, fingerprint: &str) -> TrustProject {
        TrustProject {
            request_id: "r".to_owned(),
            cwd: cwd.into(),
            fingerprint: fingerprint.into(),
        }
    }

    #[test]
    fn project_info_without_a_repository_file_needs_no_review() {
        let home = Home::new();
        let (_dir, root) = repo();
        let daemon = home.daemon();
        let info = block_on(daemon.handle_project_info(&cwd_request(&root))).unwrap();
        assert!(!info.trusted);
        assert!(!info.needs_review);
        assert!(!info.project.exists);
        assert_eq!(info.project.root, root);
        let value = serde_json::to_value(&info).unwrap();
        let keys: Vec<&str> = value.as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(&keys[keys.len() - 3..], ["trusted", "needsReview", "effective"]);
    }

    #[test]
    fn trusting_a_repository_file_requires_the_reviewed_fingerprint() {
        let home = Home::new();
        let (_dir, root) = repo();
        std::fs::write(
            Path::new(&root).join("deckhand.json"),
            "{\"devCommand\": \"npm run dev\", \"worktree\": {\"branchFrom\": \"default\"}}\n",
        )
        .unwrap();
        let daemon = home.daemon();
        let info = block_on(daemon.handle_project_info(&cwd_request(&root))).unwrap();
        assert!(info.project.exists);
        assert!(!info.trusted);
        assert!(info.needs_review);
        assert_eq!(info.effective.dev_command, None);

        let error = block_on(daemon.handle_trust_project(&trust_request(&root, "stale"))).unwrap_err();
        assert_eq!(
            error.to_string(),
            "Repository configuration changed; review it again before trusting"
        );
        assert_eq!(home.config().get("trustedProjects"), None);

        let trusted = block_on(daemon.handle_trust_project(&trust_request(&root, &info.project.fingerprint))).unwrap();
        assert!(trusted.trusted);
        assert!(!trusted.needs_review);
        assert_eq!(trusted.effective.dev_command_text(), Some("npm run dev"));
        assert_eq!(
            home.config()["trustedProjects"],
            json!({info.project.trust_root.clone(): [info.project.fingerprint.clone()]})
        );
        // Trusting again keeps one entry.
        block_on(daemon.handle_trust_project(&trust_request(&root, &info.project.fingerprint))).unwrap();
        assert_eq!(
            home.config()["trustedProjects"][&info.project.trust_root],
            json!([info.project.fingerprint])
        );

        let branches = block_on(daemon.list_branches(Some(&root))).unwrap();
        assert_eq!(branches.branch_from, BranchFrom::Default);
        assert_eq!(branches.hook, None);
    }

    #[test]
    fn invalid_global_defaults_fail_project_info() {
        let home = Home::new();
        let (_dir, root) = repo();
        std::fs::write(
            home.dir.path().join("config.json"),
            "{\"defaults\": {\"devCommand\": 5}}\n",
        )
        .unwrap();
        let daemon = home.daemon();
        let error = block_on(daemon.handle_project_info(&cwd_request(&root))).unwrap_err();
        assert!(error.to_string().starts_with("Invalid \"defaults\" in "), "{error}");
        // The branch list falls back to `current` instead.
        let branches = block_on(daemon.list_branches(Some(&root))).unwrap();
        assert_eq!(branches.branch_from, BranchFrom::Current);
    }

    #[test]
    fn list_branches_reports_branches_and_a_trusted_creation_hook() {
        let home = Home::new();
        let (_dir, root) = repo();
        tgit(&root, &["branch", "feature"]);
        let daemon = home.daemon();
        let branches = block_on(daemon.list_branches(Some(&root))).unwrap();
        let mut names = branches.branches.clone();
        names.sort();
        assert_eq!(names, ["feature", "main"]);
        assert_eq!(branches.current.as_deref(), Some("main"));
        assert_eq!(branches.default_branch.as_deref(), Some("main"));
        assert_eq!(branches.branch_from, BranchFrom::Current);
        assert_eq!(branches.hook, None);

        let scripts = Path::new(&root).join(".claude").join("scripts");
        std::fs::create_dir_all(&scripts).unwrap();
        std::fs::write(scripts.join("create-worktree.sh"), "#!/bin/sh\necho /tmp/x\n").unwrap();
        let info = block_on(daemon.handle_project_info(&cwd_request(&root))).unwrap();
        assert!(info.project.creation_hook.is_some());
        assert!(info.needs_review);
        assert_eq!(block_on(daemon.list_branches(Some(&root))).unwrap().hook, None);
        block_on(daemon.handle_trust_project(&trust_request(&root, &info.project.fingerprint))).unwrap();
        let branches = block_on(daemon.list_branches(Some(&root))).unwrap();
        assert_eq!(branches.hook, Some(true));
        let value = serde_json::to_value(&branches).unwrap();
        assert_eq!(value["hook"], json!(true));

        // A detached HEAD has no current branch.
        tgit(&root, &["checkout", "--detach"]);
        assert_eq!(block_on(daemon.list_branches(Some(&root))).unwrap().current, None);
    }

    #[test]
    fn list_branches_outside_a_repository_fails() {
        let home = Home::new();
        let dir = tempfile::tempdir().unwrap();
        let daemon = home.daemon();
        assert!(block_on(daemon.list_branches(Some(&dir.path().to_string_lossy()))).is_err());
    }

    #[test]
    fn save_config_writes_global_defaults_and_repository_files() {
        let home = Home::new();
        let (_dir, root) = repo();
        let daemon = home.daemon();
        let saved = block_on(daemon.handle_save_config(&SaveConfig {
            request_id: "r".to_owned(),
            target: ConfigTargetKind::Global,
            cwd: root.clone().into(),
            raw: Some("{\"devCommand\": \"make dev\"}\n".to_owned()),
            revision: Loose::Other(Value::Null),
        }))
        .unwrap();
        assert_eq!(saved.document.kind, ConfigTargetKind::Global);
        assert_eq!(saved.trust, None);
        assert!(saved.document.revision.is_some());
        assert_eq!(home.config()["defaults"], json!({"devCommand": "make dev"}));
        // A stale revision is refused.
        let error = block_on(daemon.handle_save_config(&SaveConfig {
            request_id: "r".to_owned(),
            target: ConfigTargetKind::Global,
            cwd: root.clone().into(),
            raw: Some("{\"devCommand\": \"other\"}\n".to_owned()),
            revision: Loose::Other(Value::Null),
        }))
        .unwrap_err();
        assert_eq!(
            error.to_string(),
            "Global defaults changed on disk. Your draft is kept; close and reopen the editor before saving."
        );

        let saved = block_on(daemon.handle_save_config(&SaveConfig {
            request_id: "r".to_owned(),
            target: ConfigTargetKind::Repository,
            cwd: root.clone().into(),
            raw: Some("{\"setupCommand\": \"npm ci\"}\n".to_owned()),
            revision: Loose::Other(Value::Null),
        }))
        .unwrap();
        assert_eq!(saved.document.kind, ConfigTargetKind::Repository);
        assert_eq!(
            std::fs::read_to_string(Path::new(&root).join("deckhand.json")).unwrap(),
            "{\"setupCommand\": \"npm ci\"}\n"
        );
        // A new file (nothing trusted to replace) is trusted as saved.
        let info = block_on(daemon.handle_project_info(&cwd_request(&root))).unwrap();
        assert!(info.trusted);
        assert_eq!(info.effective.dev_command_text(), Some("make dev"));
    }

    fn session(id: &str, repo_root: &str, updated_at: &str) -> Ordered<SessionRecord> {
        let value = json!({
            "id": id, "title": id, "program": "claude", "command": "claude", "cwd": repo_root, "repoRoot": repo_root,
            "createdAt": "2026-01-01T00:00:00.000Z", "updatedAt": updated_at, "status": "running",
        });
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn link_candidate_roots_are_most_recently_updated_first() {
        let home = Home::new();
        let daemon = home.daemon();
        {
            let mut state = daemon.sessions.borrow_mut();
            for (id, root, at) in [
                ("a", "/repo/a", "2026-01-01T00:00:00.000Z"),
                ("b", "/repo/b", "2026-03-01T00:00:00.000Z"),
                ("c", "/repo/c", "2026-01-01T00:00:00.000Z"),
                ("d", "/repo/d", "2026-02-01T00:00:00.000Z"),
            ] {
                state.sessions.insert(id.to_owned(), session(id, root, at));
            }
        }
        assert_eq!(
            daemon.link_candidate_roots(),
            ["/repo/b", "/repo/d", "/repo/a", "/repo/c"]
        );
    }
}
