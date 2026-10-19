//! Port of daemon.ts (project config, settings, trust).
//!
//! The Settings view's requests: the repository's deckhand.json and the global defaults (read, saved revision-checked,
//! never running anything), trusting a reviewed repository configuration, the worktree link candidates, and the branch
//! list the new-session flow offers.

use deckhand_core::error::Result;
use deckhand_protocol::data::{
    BranchList, CandidateSizes, LoadedProject, ProjectInfo, SavedConfigDocument, SettingsInfo, WorktreeCandidates,
};
use deckhand_protocol::records::AppConfig;
use deckhand_protocol::wire::{CwdRequest, SaveConfig, TrustProject, WorktreeCandidateSizes};

use crate::daemon::Daemon;

/// The configuration's part of the daemon state (none: configuration is read from disk for every request).
#[derive(Debug, Default)]
pub struct ConfigState {}

/// TS: daemon.ts projectInfo: `{...project, trusted, needsReview, effective}` (invalid global defaults fail here).
pub fn project_info(project: LoadedProject, config: &AppConfig) -> Result<ProjectInfo> {
    let _ = (project, config);
    todo!("D11")
}

impl Daemon {
    /// TS: handleRequest `save-config`: the global defaults, or the repository's deckhand.json (kept trusted when the
    /// version it replaces was).
    pub async fn handle_save_config(&self, request: &SaveConfig) -> Result<SavedConfigDocument> {
        let _ = request;
        todo!("D11")
    }

    /// TS: handleRequest `settings-info` (settingsInfo.ts readSettingsInfo).
    pub async fn handle_settings_info(&self, request: &CwdRequest) -> Result<SettingsInfo> {
        let _ = request;
        todo!("D11")
    }

    /// TS: handleRequest `worktree-candidates` (settingsInfo.ts readWorktreeCandidates).
    pub async fn handle_worktree_candidates(&self, request: &CwdRequest) -> Result<WorktreeCandidates> {
        let _ = request;
        todo!("D11")
    }

    /// TS: handleRequest `global-link-candidates`: candidates across every session's repository, most recently
    /// updated first.
    pub async fn handle_global_link_candidates(&self, request: &CwdRequest) -> Result<WorktreeCandidates> {
        let _ = request;
        todo!("D11")
    }

    /// TS: handleRequest `worktree-candidate-sizes` (settingsInfo.ts readCandidateSizes).
    pub async fn handle_worktree_candidate_sizes(&self, request: &WorktreeCandidateSizes) -> Result<CandidateSizes> {
        let _ = request;
        todo!("D11")
    }

    /// TS: handleRequest `project-info`.
    pub async fn handle_project_info(&self, request: &CwdRequest) -> Result<ProjectInfo> {
        let _ = request;
        todo!("D11")
    }

    /// TS: handleRequest `trust-project` (`Repository configuration changed; review it again before trusting`).
    pub async fn handle_trust_project(&self, request: &TrustProject) -> Result<ProjectInfo> {
        let _ = request;
        todo!("D11")
    }

    /// TS: InkDaemon.listBranches: local branches, current and default branch, the configured branchFrom and whether
    /// a trusted creation hook will create the worktree.
    pub async fn list_branches(&self, cwd: &str) -> Result<BranchList> {
        let _ = cwd;
        todo!("D11")
    }
}
