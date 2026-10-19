//! Port of settingsInfo.ts.
//!
//! Daemon-side readers for the Settings screen (C): the effective settings with their sources plus both editable
//! documents, the link candidates of the main checkout (or, for the Global column, the paths shared by the user's
//! repositories) and their bounded sizes. Saving goes through save-config. The records are deckhand-protocol's.

use std::path::Path;

use deckhand_core::error::Result;
use deckhand_protocol::Ordered;
use deckhand_protocol::data::{
    CandidateKind, CandidateSizes, CandidateSuggestion, CodexHookState, CodexHookStatus, SettingsInfo,
    WorktreeCandidate, WorktreeCandidates, WorktreeSettings,
};

/// TS: settingsInfo.ts MAX_CANDIDATES (candidates listed at most; the rest are counted).
pub const MAX_CANDIDATES: usize = 200;
/// TS: settingsInfo.ts MAX_LSTATS (entries whose kind is checked at most).
pub const MAX_LSTATS: usize = 2000;
/// TS: settingsInfo.ts MAX_REPOS (repositories scanned for global link candidates).
pub const MAX_REPOS: usize = 12;
/// TS: settingsInfo.ts MAX_KNOWN_ROOTS (session roots considered for global link candidates).
pub const MAX_KNOWN_ROOTS: usize = 40;
/// TS: settingsInfo.ts MAX_SIZE_PATHS (paths per size request).
pub const MAX_SIZE_PATHS: usize = 24;
/// TS: settingsInfo.ts SIZE_TIMEOUT_MS (per `du`).
pub const SIZE_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(4000);
/// TS: settingsInfo.ts SIZE_CONCURRENCY.
pub const SIZE_CONCURRENCY: usize = 4;
/// TS: settingsInfo.ts LINK_SEGMENTS (path segments suggested as links: dependencies).
pub const LINK_SEGMENTS: [&str; 5] = ["node_modules", ".venv", "venv", "vendor", ".vercel"];
/// TS: settingsInfo.ts SKIP_SEGMENTS (segment → why it is not suggested), in TS key order.
pub const SKIP_SEGMENTS: [(&str, &str); 10] = [
    ("dist", "build output"),
    ("build", "build output"),
    ("out", "build output"),
    ("coverage", "coverage"),
    (".next", "build cache"),
    (".turbo", "build cache"),
    (".cache", "cache"),
    ("__pycache__", "cache"),
    (".pytest_cache", "cache"),
    (".DS_Store", "OS clutter"),
];

/// TS: settingsInfo.ts codexHookCommands (every hook `command` in Codex's hooks.json and config.toml texts; TOML
/// strings read loosely, as a hint).
pub fn codex_hook_commands(hooks_json: Option<&str>, config_toml: Option<&str>) -> Vec<String> {
    let _ = (hooks_json, config_toml);
    todo!("I7")
}

/// TS: settingsInfo.ts codexHookState (`None`: `commands` include `expected`, this Deckhand's bridge; `Other`:
/// another Deckhand bridge; `Missing`: none).
pub fn codex_hook_state(commands: &[String], expected: &str) -> Option<CodexHookState> {
    let _ = (commands, expected);
    todo!("I7")
}

/// TS: settingsInfo.ts readCodexHooks (`$CODEX_HOME` or `~/.codex`; `None` when Codex is not used here or its hooks
/// call this Deckhand).
pub async fn read_codex_hooks() -> Option<CodexHookStatus> {
    todo!("I7")
}

/// TS: settingsInfo.ts readSettingsInfo (C → Settings for `cwd`'s repository; read-only).
pub async fn read_settings_info(cwd: &str) -> Result<SettingsInfo> {
    let _ = cwd;
    todo!("I7")
}

/// TS: settingsInfo.ts classifyCandidate result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CandidateClass {
    pub suggestion: CandidateSuggestion,
    /// Why (`dependencies`, `env file`, `build output`, `log`, ...; empty for everything else).
    pub reason: String,
}

/// TS: settingsInfo.ts classifyCandidate (dependencies and env files are suggested as links; build output, caches,
/// clutter, logs and everything else are skipped).
pub fn classify_candidate(relative: &str) -> CandidateClass {
    let _ = relative;
    todo!("I7")
}

/// TS: settingsInfo.ts entryKind result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EntryKind {
    pub kind: CandidateKind,
    /// A symlink's real path (`None` when broken).
    pub target: Option<String>,
}

/// TS: settingsInfo.ts entryKind (lstat; `None` when missing).
pub async fn entry_kind(file: &Path) -> Option<EntryKind> {
    let _ = file;
    todo!("I7")
}

/// TS: settingsInfo.ts Layers (both layers' worktree settings as written, trusted or not).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Layers {
    pub global: Option<WorktreeSettings>,
    pub repository: Option<WorktreeSettings>,
}

/// TS: settingsInfo.ts readCandidates result.
#[derive(Clone, Debug, PartialEq)]
pub struct CandidateList {
    pub candidates: Vec<Ordered<WorktreeCandidate>>,
    /// Entries not listed (past MAX_CANDIDATES).
    pub more: i64,
}

/// TS: settingsInfo.ts readCandidates (untracked and ignored entries of `checkout`, directories collapsed, plus the
/// configured entries, configured first).
pub async fn read_candidates(checkout: &str, layers: &Layers) -> Result<CandidateList> {
    let _ = (checkout, layers);
    todo!("I7")
}

/// TS: settingsInfo.ts readWorktreeCandidates (Settings → Linked items for the repository column; a scan failure is
/// `candidatesError`).
pub async fn read_worktree_candidates(cwd: &str) -> Result<WorktreeCandidates> {
    let _ = cwd;
    todo!("I7")
}

/// TS: settingsInfo.ts readGlobalLinkCandidates (Settings → Linked items · Global: the global entries and the paths
/// shared by at least two of the user's repositories, from `cwd` and `known_roots`).
pub async fn read_global_link_candidates(cwd: &str, known_roots: &[String]) -> Result<WorktreeCandidates> {
    let _ = (cwd, known_roots);
    todo!("I7")
}

/// TS: settingsInfo.ts du (`du -sk -- <file>` in KiB; `None` when unknown or timed out).
pub async fn du(file: &Path) -> Option<i64> {
    let _ = file;
    todo!("I7")
}

/// TS: settingsInfo.ts readCandidateSizes (KiB per relative path in the main checkout; symlinks 0; `Pass at most 24
/// relative candidate paths`).
pub async fn read_candidate_sizes(cwd: &str, paths: &[String]) -> Result<CandidateSizes> {
    let _ = (cwd, paths);
    todo!("I7")
}
