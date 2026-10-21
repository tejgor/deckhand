//! Port of settingsInfo.ts.
//!
//! Daemon-side readers for the Settings screen (C): the effective settings with their sources plus both editable
//! documents, the link candidates of the main checkout (or, for the Global column, the paths shared by the user's
//! repositories) and their bounded sizes. Saving goes through save-config. The records are deckhand-protocol's.

use std::collections::HashSet;
use std::path::Path;

use futures::StreamExt;
use indexmap::{IndexMap, IndexSet};
use serde_json::{Map, Value};

use deckhand_core::error::{DhError, Result, json_parse_message};
use deckhand_core::jsstr::{self, path};
use deckhand_core::project_config::{
    ExplainContext, PROJECT_CONFIG_FILE, explain_settings, is_project_trusted, project_needs_review,
    validate_project_config,
};
use deckhand_core::worktree_links::{merge_worktree_settings, valid_relative_path};
use deckhand_protocol::data::{
    CandidateConfigured, CandidateKind, CandidateSizes, CandidateSuggestion, CodexHookState, CodexHookStatus,
    ConfigTargetKind, LoadedProject, RepositoryInfo, RepositoryState, SettingsInfo, WorktreeCandidate,
    WorktreeCandidates, WorktreeSettings,
};
use deckhand_protocol::{AppConfig, KeyOrder, Ordered};

use crate::agent_path::hook_command;
use crate::config::load_project_config;
use crate::config_document::read_config_targets;
use crate::git::{
    BranchWhere, GitOptions, RepoContext, git, optional_git, resolve_create_script, resolve_default_branch,
    resolve_repo_context, worktree_template_vars,
};
use crate::paths;
use crate::proc::{self, ExecOptions};
use crate::storage::load_app_config;
use crate::workspace_git::parse_status;
use crate::worktree_links::user_slug;

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

/// TS: settingsInfo.ts UNTRACKED_STATUS (untracked and ignored entries, directories collapsed).
const UNTRACKED_STATUS: [&str; 6] = [
    "--no-optional-locks",
    "status",
    "--porcelain=v2",
    "-z",
    "--ignored=matching",
    "--untracked-files=normal",
];
/// TS: `{maxBuffer: 32 * 1024 * 1024}` for the status scans.
const STATUS_MAX_BUFFER: usize = 32 * 1024 * 1024;
/// TS: du `maxBuffer`.
const DU_MAX_BUFFER: usize = 64 * 1024;

// ---------------------------------------------------------------------------------------------------------------
// Codex hooks

/// A JS array index key (canonical decimal below 2^32 - 1), which `Object.entries` lists first, ascending.
fn array_index(key: &str) -> Option<u32> {
    if key.is_empty() || (key.len() > 1 && key.starts_with('0')) || !key.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    key.parse::<u32>().ok().filter(|index| *index != u32::MAX)
}

/// `Object.entries(object)`: array indices ascending, then the other keys in insertion order.
fn js_entries(map: &Map<String, Value>) -> Vec<(&String, &Value)> {
    let mut entries: Vec<(&String, &Value)> = map.iter().collect();
    entries.sort_by_key(|(key, _)| array_index(key).map_or((1, 0), |index| (0, index)));
    entries
}

/// `\uD800`–`\uDFFF` escapes without their pair, which V8 keeps as lone surrogates, as `�` (Rust strings cannot
/// hold them). Only called on text V8 accepts, so every escape is well formed.
fn replace_lone_surrogate_escapes(text: &str) -> String {
    fn escape_at(chars: &[char], at: usize) -> Option<u32> {
        if chars.get(at) != Some(&'\\') || chars.get(at + 1) != Some(&'u') {
            return None;
        }
        let hex: String = chars.get(at + 2..at + 6)?.iter().collect();
        u32::from_str_radix(&hex, 16).ok()
    }
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut in_string = false;
    let mut i = 0;
    while let Some(&ch) = chars.get(i) {
        if !in_string {
            in_string = ch == '"';
            out.push(ch);
            i += 1;
            continue;
        }
        match ch {
            '"' => {
                in_string = false;
                out.push(ch);
                i += 1;
            }
            '\\' => match escape_at(&chars, i) {
                Some(0xD800..=0xDBFF) => {
                    if escape_at(&chars, i + 6).is_some_and(|low| (0xDC00..=0xDFFF).contains(&low)) {
                        out.extend(chars.get(i..i + 12).unwrap_or_default());
                        i += 12;
                    } else {
                        out.push_str("\\ufffd");
                        i += 6;
                    }
                }
                Some(0xDC00..=0xDFFF) => {
                    out.push_str("\\ufffd");
                    i += 6;
                }
                _ => {
                    out.extend(chars.get(i..i + 2).unwrap_or_default());
                    i += 2;
                }
            },
            _ => {
                out.push(ch);
                i += 1;
            }
        }
    }
    out
}

/// `JSON.parse(text)`, `None` where it throws. What V8 accepts but serde_json does not: lone surrogate escapes (read
/// as U+FFFD) and numbers out of range (given up on, as only strings matter here).
fn js_json_parse(text: &str) -> Option<Value> {
    if json_parse_message(text).is_some() {
        return None;
    }
    serde_json::from_str(text)
        .ok()
        .or_else(|| serde_json::from_str(&replace_lone_surrogate_escapes(text)).ok())
}

/// JS LineTerminator (what `.` does not match and multiline `^` follows).
fn is_line_terminator(ch: char) -> bool {
    matches!(ch, '\n' | '\r' | '\u{2028}' | '\u{2029}')
}

/// `/\s*command\s*=\s*("(?:[^"\\]|\\.)*"|'[^']*')/` anchored at `text[start..]`: the quoted value and the end.
fn toml_command_at(text: &str, start: usize) -> Option<(&str, usize)> {
    let skip_ws = |at: usize| -> usize {
        let tail = text.get(at..).unwrap_or_default();
        at + (tail.len() - tail.trim_start_matches(jsstr::is_js_whitespace).len())
    };
    let at = skip_ws(start);
    let at = at + text.get(at..)?.strip_prefix("command").map(|_| "command".len())?;
    let at = skip_ws(at);
    let at = at + text.get(at..)?.strip_prefix('=').map(|_| 1)?;
    let open = skip_ws(at);
    let body = text.get(open..)?;
    let mut chars = body.char_indices();
    let quote = chars.next()?.1;
    match quote {
        '"' => {
            while let Some((index, ch)) = chars.next() {
                match ch {
                    '"' => return Some((&body[..=index], open + index + 1)),
                    '\\' => match chars.next() {
                        Some((_, escaped)) if !is_line_terminator(escaped) => {}
                        _ => return None,
                    },
                    _ => {}
                }
            }
            None
        }
        '\'' => {
            let close = body.get(1..)?.find('\'')? + 1;
            Some((&body[..=close], open + close + 1))
        }
        _ => None,
    }
}

/// TS: settingsInfo.ts codexHookCommands (every hook `command` in Codex's hooks.json and config.toml texts; TOML
/// strings read loosely, as a hint).
pub fn codex_hook_commands(hooks_json: Option<&str>, config_toml: Option<&str>) -> Vec<String> {
    fn walk(value: &Value, commands: &mut Vec<String>) {
        match value {
            Value::Array(items) => items.iter().for_each(|item| walk(item, commands)),
            Value::Object(map) => {
                for (key, child) in js_entries(map) {
                    match child {
                        Value::String(command) if key == "command" => commands.push(command.clone()),
                        _ => walk(child, commands),
                    }
                }
            }
            _ => {}
        }
    }
    let mut commands = Vec::new();
    if let Some(value) = hooks_json.filter(|text| !text.is_empty()).and_then(js_json_parse) {
        walk(&value, &mut commands);
    }
    // matchAll(/^\s*command\s*=\s*(...)/gm): tried at every line start from where the previous match ended.
    let toml = config_toml.unwrap_or_default();
    let line_starts = std::iter::once(0).chain(
        toml.char_indices()
            .filter(|(_, ch)| is_line_terminator(*ch))
            .map(|(index, ch)| index + ch.len_utf8()),
    );
    let mut cursor = 0;
    for start in line_starts {
        if start < cursor {
            continue;
        }
        let Some((quoted, end)) = toml_command_at(toml, start) else {
            continue;
        };
        cursor = end;
        if let Some(inner) = quoted.strip_prefix('\'') {
            commands.push(inner.strip_suffix('\'').unwrap_or(inner).to_owned());
        } else if let Some(Value::String(command)) = js_json_parse(quoted) {
            commands.push(command);
        }
    }
    commands
}

/// `/\bhook'?\s*$/`: `hook` as a word ending the command (an optional closing quote and whitespace after it).
fn ends_with_hook_word(command: &str) -> bool {
    let trimmed = command.trim_end_matches(jsstr::is_js_whitespace);
    let trimmed = trimmed.strip_suffix('\'').unwrap_or(trimmed);
    trimmed.strip_suffix("hook").is_some_and(|before| {
        !before
            .chars()
            .next_back()
            .is_some_and(|ch| ch.is_ascii_alphanumeric() || ch == '_')
    })
}

/// TS: settingsInfo.ts codexHookState (`None`: `commands` include `expected`, this Deckhand's bridge; `Other`:
/// another Deckhand bridge; `Missing`: none).
pub fn codex_hook_state(commands: &[String], expected: &str) -> Option<CodexHookState> {
    if commands.iter().any(|command| command == expected) {
        return None;
    }
    // `/deckhand/i` without the u flag folds ASCII only.
    let other = commands
        .iter()
        .any(|command| command.to_ascii_lowercase().contains("deckhand") && ends_with_hook_word(command));
    Some(if other {
        CodexHookState::Other
    } else {
        CodexHookState::Missing
    })
}

/// `fs.readFile(file, 'utf8')`, `None` on any error.
async fn read_text(file: &Path) -> Option<String> {
    tokio::fs::read(file)
        .await
        .ok()
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
}

/// TS: settingsInfo.ts readCodexHooks (`$CODEX_HOME` or `~/.codex`; `None` when Codex is not used here or its hooks
/// call this Deckhand).
pub async fn read_codex_hooks() -> Option<CodexHookStatus> {
    let dir = match std::env::var_os("CODEX_HOME") {
        Some(home) if !home.is_empty() => std::path::PathBuf::from(home),
        _ => paths::home_dir().join(".codex"),
    };
    // No Codex home: Codex isn't used here, so there is nothing to hint.
    if !tokio::fs::metadata(&dir).await.is_ok_and(|meta| meta.is_dir()) {
        return None;
    }
    let file = dir.join("hooks.json");
    let config = dir.join("config.toml");
    let (hooks_json, config_toml) = tokio::join!(read_text(&file), read_text(&config));
    let commands = codex_hook_commands(hooks_json.as_deref(), config_toml.as_deref());
    let state = codex_hook_state(&commands, &hook_command().ok()?)?;
    let command = if paths::is_dev_channel() {
        "scripts/deckhand-dev.sh hooks codex"
    } else {
        "deckhand hooks codex"
    };
    Some(CodexHookStatus {
        state,
        file: file.to_string_lossy().into_owned(),
        file_exists: hooks_json.is_some(),
        command: command.to_owned(),
    })
}

// ---------------------------------------------------------------------------------------------------------------
// Settings

/// TS: `user.defaults ?? {}` validated as the global layer (`None` when invalid or without `worktree`).
fn global_worktree(user: &AppConfig) -> Option<WorktreeSettings> {
    let defaults = match &user.defaults {
        None | Some(Value::Null) => Value::Object(Map::new()),
        Some(value) => value.clone(),
    };
    validate_project_config(&defaults, Some("defaults"))
        .ok()
        .and_then(|config| config.into_inner().worktree)
}

/// TS: settingsInfo.ts isProjectTrusted(project, user).
fn project_trusted(project: &LoadedProject, user: &AppConfig) -> bool {
    is_project_trusted(
        &project.trust_root,
        &project.fingerprint,
        user.trusted_projects.as_ref(),
    )
}

/// TS: settingsInfo.ts readSettingsInfo (C → Settings for `cwd`'s repository; read-only).
pub async fn read_settings_info(cwd: &str) -> Result<SettingsInfo> {
    let user = load_app_config().await?;
    let mut error: Option<String> = None;
    let context: Option<RepoContext> = match resolve_repo_context(cwd).await {
        Ok(context) => Some(context),
        Err(caught) => {
            error = Some(format!("Not a Git repository: {caught}"));
            None
        }
    };
    let mut project: Option<LoadedProject> = None;
    if let Some(context) = &context {
        match load_project_config(cwd, Some(&user), Some(context)).await {
            Ok(loaded) => project = Some(loaded),
            Err(caught) => error = Some(caught.to_string()),
        }
    }
    let checkout = context
        .as_ref()
        .map(|context| context.main_root.clone().unwrap_or_else(|| context.root.clone()));
    let main_root = context.as_ref().and_then(|context| context.main_root.clone());

    let hook_file = async {
        match (&project, &context) {
            (Some(project), _) => project
                .creation_hook
                .as_ref()
                .map(|hook| hook.file.clone())
                .or_else(|| project.disabled_hook.as_ref().map(|hook| hook.file.clone())),
            (None, Some(context)) => resolve_create_script(&context.root, context.main_root.as_deref()).await,
            (None, None) => None,
        }
    };
    let vars = async {
        match &context {
            Some(_) => worktree_template_vars("<name>", cwd).await.ok(),
            None => None,
        }
    };
    let inside_ignored = async {
        match &main_root {
            Some(main_root) => optional_git(main_root, &["check-ignore", "-q", "--", ".worktrees/probe"], None)
                .await
                .is_some(),
            None => false,
        }
    };
    let default_branch = async {
        match &checkout {
            Some(checkout) => resolve_default_branch(checkout, None, BranchWhere::Local).await,
            None => None,
        }
    };
    let remotes = async {
        match &checkout {
            Some(checkout) => optional_git(checkout, &["remote"], None).await,
            None => None,
        }
    };
    let (hook_file, vars, targets, inside_ignored, default_branch, remotes) = tokio::join!(
        hook_file,
        vars,
        read_config_targets(cwd),
        inside_ignored,
        default_branch,
        remotes
    );
    let codex_hooks = async {
        if user.agent_hooks == Some(true) {
            read_codex_hooks().await
        } else {
            None
        }
    };
    // Not in the TS: Settings says when macOS blocks Deckhand's notifications or never got an answer.
    let notification_permission = async {
        if user.notifications == Some(true) {
            crate::notify::permission(true).await
        } else {
            None
        }
    };
    let (codex_hooks, notification_permission) = tokio::join!(codex_hooks, notification_permission);
    let has_origin = remotes
        .as_deref()
        .is_some_and(|remotes| remotes.split('\n').any(|remote| remote == "origin"));
    let origin_branch = match (&checkout, has_origin) {
        (Some(checkout), true) => resolve_default_branch(checkout, Some("origin"), BranchWhere::Remote).await,
        _ => None,
    };
    let hook_file = hook_file.filter(|file| !file.is_empty());
    let config_dir = paths::config_dir().to_string_lossy().into_owned();
    let explanation = explain_settings(
        project.as_ref(),
        &user,
        &ExplainContext {
            vars: vars.clone(),
            user: Some(user_slug()),
            hook_file: hook_file.clone(),
            config_dir: config_dir.clone(),
            location_config_dir: None,
        },
    );
    let state = match (&context, &error, &project) {
        (None, _, _) => RepositoryState::None,
        (_, Some(_), _) => RepositoryState::Invalid,
        (_, None, Some(project)) if project.path.is_none() => RepositoryState::Bare,
        (_, None, Some(project)) if !project.exists => RepositoryState::Absent,
        (_, None, Some(project)) if project_trusted(project, &user) => RepositoryState::Trusted,
        _ => RepositoryState::Untrusted,
    };
    let file = project.as_ref().and_then(|project| project.path.clone()).or_else(|| {
        main_root
            .as_deref()
            .map(|main| path::join(&[main, PROJECT_CONFIG_FILE]))
    });
    Ok(SettingsInfo {
        cwd: cwd.to_owned(),
        repo: vars
            .as_ref()
            .map(|vars| vars.repo.clone())
            .unwrap_or_else(|| path::basename(cwd)),
        repository: RepositoryInfo {
            state,
            path: file.filter(|file| !file.is_empty()),
            error: error.filter(|error| !error.is_empty()),
        },
        needs_review: project
            .as_ref()
            .is_some_and(|project| project_needs_review(project, user.trusted_projects.as_ref())),
        global_error: explanation.global_error.filter(|error| !error.is_empty()),
        rows: explanation.rows,
        targets,
        hook_file,
        vars,
        default_location: path::join(&[&config_dir, "worktrees", "<name>"]),
        inside_ignored,
        default_branch: default_branch.filter(|branch| !branch.is_empty()),
        origin_branch: origin_branch.filter(|branch| !branch.is_empty()),
        user: user_slug(),
        agent_hooks: user.agent_hooks,
        notifications: user.notifications == Some(true),
        scroll_speed: user.scroll_speed.map(|value| value.0),
        codex_hooks,
        notification_permission,
    })
}

// ---------------------------------------------------------------------------------------------------------------
// Link candidates

/// TS: settingsInfo.ts classifyCandidate result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CandidateClass {
    pub suggestion: CandidateSuggestion,
    /// Why (`dependencies`, `env file`, `build output`, `log`, ...; empty for everything else).
    pub reason: String,
}

/// TS: settingsInfo.ts isEnvName.
fn is_env_name(segment: &str) -> bool {
    segment == ".env" || segment.starts_with(".env.") || segment.ends_with(".env")
}

/// TS: settingsInfo.ts classifyCandidate (dependencies and env files are suggested as links; build output, caches,
/// clutter, logs and everything else are skipped).
pub fn classify_candidate(relative: &str) -> CandidateClass {
    let class = |suggestion, reason: &str| CandidateClass {
        suggestion,
        reason: reason.to_owned(),
    };
    let segments: Vec<&str> = relative.split('/').collect();
    if segments.iter().any(|segment| LINK_SEGMENTS.contains(segment)) {
        return class(CandidateSuggestion::Link, "dependencies");
    }
    if segments.iter().any(|segment| is_env_name(segment)) {
        return class(CandidateSuggestion::Link, "env file");
    }
    let skip = segments
        .iter()
        .find_map(|segment| SKIP_SEGMENTS.iter().find(|(name, _)| name == segment));
    if let Some((_, reason)) = skip {
        return class(CandidateSuggestion::Skip, reason);
    }
    if segments.last().is_some_and(|last| last.ends_with(".log")) {
        return class(CandidateSuggestion::Skip, "log");
    }
    class(CandidateSuggestion::Skip, "")
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
    let meta = tokio::fs::symlink_metadata(file).await.ok()?;
    if meta.file_type().is_symlink() {
        let target = tokio::fs::canonicalize(file)
            .await
            .ok()
            .map(|real| real.to_string_lossy().into_owned());
        return Some(EntryKind {
            kind: CandidateKind::Symlink,
            target,
        });
    }
    Some(EntryKind {
        kind: if meta.is_dir() {
            CandidateKind::Dir
        } else {
            CandidateKind::File
        },
        target: None,
    })
}

/// TS: settingsInfo.ts Layers (both layers' worktree settings as written, trusted or not).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Layers {
    pub global: Option<WorktreeSettings>,
    pub repository: Option<WorktreeSettings>,
}

impl Layers {
    fn get(&self, kind: ConfigTargetKind) -> Option<&WorktreeSettings> {
        match kind {
            ConfigTargetKind::Global => self.global.as_ref(),
            ConfigTargetKind::Repository => self.repository.as_ref(),
        }
    }
}

/// TS: settingsInfo.ts readCandidates result.
#[derive(Clone, Debug, PartialEq)]
pub struct CandidateList {
    pub candidates: Vec<Ordered<WorktreeCandidate>>,
    /// Entries not listed (past MAX_CANDIDATES).
    pub more: i64,
}

/// `entry.replace(/\/$/, '')`.
fn strip_slash(entry: &str) -> &str {
    entry.strip_suffix('/').unwrap_or(entry)
}

/// A candidate with every optional key absent.
fn candidate(path: &str, kind: CandidateKind, ignored: bool, class: CandidateClass) -> WorktreeCandidate {
    WorktreeCandidate {
        path: path.to_owned(),
        kind,
        target: None,
        ignored,
        suggestion: class.suggestion,
        reason: class.reason,
        configured: None,
        source: None,
        layers: None,
        repos: None,
        missing: None,
        added: None,
    }
}

/// What a configured entry adds (TS `Pick<WorktreeCandidate, 'configured' | 'layers' | 'source'>`).
struct Configured {
    configured: CandidateConfigured,
    layers: Vec<ConfigTargetKind>,
    source: Option<String>,
}

const LAYER_KINDS: [ConfigTargetKind; 2] = [ConfigTargetKind::Global, ConfigTargetKind::Repository];

/// TS: settingsInfo.ts readCandidates (untracked and ignored entries of `checkout`, directories collapsed, plus the
/// configured entries, configured first).
pub async fn read_candidates(checkout: &str, layers: &Layers) -> Result<CandidateList> {
    let settings = merge_worktree_settings(layers.global.as_ref(), layers.repository.as_ref()).unwrap_or_default();
    let options = GitOptions::default().with_max_buffer(STATUS_MAX_BUFFER);
    let status = parse_status(&git(checkout, &UNTRACKED_STATUS, Some(&options)).await?);
    let mut configured: IndexMap<String, Configured> = IndexMap::new();
    for entry in settings.symlink.iter().flatten() {
        let listing = LAYER_KINDS
            .into_iter()
            .filter(|kind| {
                layers
                    .get(*kind)
                    .and_then(|layer| layer.symlink.as_ref())
                    .is_some_and(|symlink| symlink.contains(entry))
            })
            .collect();
        configured.insert(
            entry.clone(),
            Configured {
                configured: CandidateConfigured::Symlink,
                layers: listing,
                source: None,
            },
        );
    }
    for (destination, source) in settings.files.iter().flatten() {
        let listing = LAYER_KINDS
            .into_iter()
            .filter(|kind| {
                layers
                    .get(*kind)
                    .and_then(|layer| layer.files.as_ref())
                    .is_some_and(|files| files.contains_key(destination))
            })
            .collect();
        configured.insert(
            destination.clone(),
            Configured {
                configured: CandidateConfigured::Files,
                layers: listing,
                source: Some(source.clone()),
            },
        );
    }
    let mut entries: Vec<(String, bool, CandidateClass)> = status
        .untracked
        .iter()
        .map(|file| (file, false))
        .chain(status.ignored.iter().map(|file| (file, true)))
        .map(|(file, ignored)| (strip_slash(file).to_owned(), ignored))
        .filter(|(file, _)| valid_relative_path(file) && file != PROJECT_CONFIG_FILE && !configured.contains_key(file))
        .map(|(file, ignored)| {
            let class = classify_candidate(&file);
            (file, ignored, class)
        })
        .collect();
    let is_link = |class: &CandidateClass| class.suggestion == CandidateSuggestion::Link;
    entries.sort_by(|a, b| {
        is_link(&b.2)
            .cmp(&is_link(&a.2))
            .then_with(|| jsstr::locale_compare(&a.0, &b.0))
    });
    let shown = entries.iter().take(MAX_CANDIDATES);
    let mut candidates = Vec::new();
    for (file, config) in &configured {
        let kind = entry_kind(&Path::new(checkout).join(file)).await;
        let ignored = status.ignored.iter().any(|entry| strip_slash(entry) == file);
        let mut item = candidate(
            file,
            kind.as_ref().map_or(CandidateKind::File, |kind| kind.kind),
            ignored,
            classify_candidate(file),
        );
        item.target = kind
            .as_ref()
            .and_then(|kind| kind.target.clone())
            .filter(|target| !target.is_empty());
        item.configured = Some(config.configured);
        item.source = config.source.clone();
        item.layers = Some(config.layers.clone());
        item.missing = kind.is_none().then_some(true);
        candidates.push(Ordered::new(item));
    }
    let mut checks = 0;
    for (file, ignored, class) in shown {
        checks += 1;
        let kind = if checks <= MAX_LSTATS {
            entry_kind(&Path::new(checkout).join(file)).await
        } else {
            None
        };
        let mut item = candidate(
            file,
            kind.as_ref().map_or(CandidateKind::File, |kind| kind.kind),
            *ignored,
            class.clone(),
        );
        item.target = kind.and_then(|kind| kind.target).filter(|target| !target.is_empty());
        candidates.push(Ordered::new(item));
    }
    Ok(CandidateList {
        candidates,
        more: entries.len().saturating_sub(MAX_CANDIDATES) as i64,
    })
}

/// `message(error).split('\n')[0]`.
fn first_line(error: &DhError) -> String {
    error.to_string().split('\n').next().unwrap_or_default().to_owned()
}

/// TS: settingsInfo.ts readWorktreeCandidates (Settings → Linked items for the repository column; a scan failure is
/// `candidatesError`).
pub async fn read_worktree_candidates(cwd: &str) -> Result<WorktreeCandidates> {
    let context = resolve_repo_context(cwd).await?;
    let user = load_app_config().await?;
    let checkout = context.main_root.clone().unwrap_or_else(|| context.root.clone());
    // Both layers as written, trusted or not: the picker edits files.
    let layers = Layers {
        global: global_worktree(&user),
        repository: load_project_config(cwd, Some(&user), Some(&context))
            .await
            .ok()
            .and_then(|project| project.config.into_inner().worktree),
    };
    Ok(match read_candidates(&checkout, &layers).await {
        Ok(read) => WorktreeCandidates {
            checkout,
            candidates: read.candidates,
            more_candidates: read.more,
            repos: None,
            candidates_error: None,
        },
        Err(error) => WorktreeCandidates {
            checkout,
            candidates: Vec::new(),
            more_candidates: 0,
            repos: None,
            candidates_error: Some(first_line(&error)),
        },
    })
}

/// One repository scanned for global link candidates: its untracked/ignored entries (slash stripped → ignored).
struct ScannedRepo {
    root: String,
    entries: IndexMap<String, bool>,
}

/// TS: settingsInfo.ts readGlobalLinkCandidates (Settings → Linked items · Global: the global entries and the paths
/// shared by at least two of the user's repositories, from `cwd` and `known_roots`).
pub async fn read_global_link_candidates(cwd: &str, known_roots: &[String]) -> Result<WorktreeCandidates> {
    let user = load_app_config().await?;
    let global = global_worktree(&user);
    let roots: IndexSet<&str> = std::iter::once(cwd)
        .chain(known_roots.iter().map(String::as_str))
        .collect();
    let roots: Vec<&str> = roots.into_iter().take(MAX_KNOWN_ROOTS).collect();
    let resolved = futures::future::join_all(roots.iter().map(|root| async move {
        resolve_repo_context(root)
            .await
            .ok()
            .map(|context| context.main_root.unwrap_or(context.root))
    }))
    .await;
    let checkouts: IndexSet<String> = resolved
        .into_iter()
        .flatten()
        .filter(|checkout| !checkout.is_empty())
        .collect();
    let checkouts: Vec<String> = checkouts.into_iter().take(MAX_REPOS).collect();
    let checkout = checkouts.first().cloned().unwrap_or_else(|| cwd.to_owned());
    let options = GitOptions::default().with_max_buffer(STATUS_MAX_BUFFER);
    let scanned = futures::future::join_all(checkouts.iter().map(|root| {
        let options = &options;
        async move {
            let status = parse_status(&git(root, &UNTRACKED_STATUS, Some(options)).await.ok()?);
            let mut entries = IndexMap::new();
            for (file, ignored) in status
                .untracked
                .iter()
                .map(|file| (file, false))
                .chain(status.ignored.iter().map(|file| (file, true)))
            {
                entries.insert(strip_slash(file).to_owned(), ignored);
            }
            Some(ScannedRepo {
                root: root.clone(),
                entries,
            })
        }
    }))
    .await;
    let repos: Vec<ScannedRepo> = scanned.into_iter().flatten().collect();
    let name = |repo: &ScannedRepo| path::basename(&repo.root);
    let mut candidates: Vec<Ordered<WorktreeCandidate>> = Vec::new();
    let mut configured: HashSet<String> = HashSet::new();
    for entry in global
        .as_ref()
        .and_then(|global| global.symlink.as_ref())
        .into_iter()
        .flatten()
    {
        configured.insert(entry.clone());
        let mut having: Vec<&ScannedRepo> = Vec::new();
        for repo in &repos {
            if entry_kind(&Path::new(&repo.root).join(entry)).await.is_some() {
                having.push(repo);
            }
        }
        let kind = match having.first() {
            Some(first) => entry_kind(&Path::new(&first.root).join(entry)).await,
            None => None,
        };
        let ignored = having
            .first()
            .is_some_and(|first| first.entries.get(entry).copied().unwrap_or(false));
        let mut item = candidate(
            entry,
            kind.map_or(CandidateKind::File, |kind| kind.kind),
            ignored,
            classify_candidate(entry),
        );
        item.configured = Some(CandidateConfigured::Symlink);
        item.layers = Some(vec![ConfigTargetKind::Global]);
        item.repos = Some(having.iter().map(|repo| name(repo)).collect());
        item.missing = having.is_empty().then_some(true);
        candidates.push(Ordered::new(item));
    }
    for (destination, source) in global
        .as_ref()
        .and_then(|global| global.files.as_ref())
        .into_iter()
        .flatten()
    {
        configured.insert(destination.clone());
        let mut item = candidate(destination, CandidateKind::File, false, classify_candidate(destination));
        item.configured = Some(CandidateConfigured::Files);
        item.layers = Some(vec![ConfigTargetKind::Global]);
        item.source = Some(source.clone());
        // TS literal order: ..., configured, layers, source.
        let order = KeyOrder::Object(
            [
                "path",
                "kind",
                "ignored",
                "suggestion",
                "reason",
                "configured",
                "layers",
                "source",
            ]
            .into_iter()
            .map(|key| (key.to_owned(), KeyOrder::Unrecorded))
            .collect(),
        );
        candidates.push(Ordered::with_order(item, order));
    }
    let mut shared: IndexMap<&str, Vec<&ScannedRepo>> = IndexMap::new();
    for repo in &repos {
        for file in repo.entries.keys() {
            if !valid_relative_path(file) || file == PROJECT_CONFIG_FILE || configured.contains(file) {
                continue;
            }
            shared.entry(file.as_str()).or_default().push(repo);
        }
    }
    let mut suggestions: Vec<(&str, Vec<&ScannedRepo>, CandidateClass)> = shared
        .into_iter()
        .filter(|(_, having)| having.len() >= 2)
        .map(|(file, having)| (file, having, classify_candidate(file)))
        .filter(|(_, _, class)| class.suggestion == CandidateSuggestion::Link || class.reason.is_empty())
        .collect();
    let is_link = |class: &CandidateClass| class.suggestion == CandidateSuggestion::Link;
    suggestions.sort_by(|a, b| {
        b.1.len()
            .cmp(&a.1.len())
            .then_with(|| is_link(&b.2).cmp(&is_link(&a.2)))
            .then_with(|| jsstr::locale_compare(a.0, b.0))
    });
    for (file, having, class) in suggestions.iter().take(MAX_CANDIDATES) {
        let Some(first) = having.first() else {
            continue;
        };
        let kind = entry_kind(&Path::new(&first.root).join(file)).await;
        let ignored = first.entries.get(*file).copied().unwrap_or(false);
        let mut item = candidate(
            file,
            kind.map_or(CandidateKind::File, |kind| kind.kind),
            ignored,
            class.clone(),
        );
        item.repos = Some(having.iter().map(|repo| name(repo)).collect());
        candidates.push(Ordered::new(item));
    }
    let more_candidates = suggestions.len().saturating_sub(MAX_CANDIDATES) as i64;
    Ok(WorktreeCandidates {
        checkout,
        candidates,
        more_candidates,
        repos: Some(repos.iter().map(name).collect()),
        candidates_error: repos.is_empty().then(|| "no repository could be read".to_owned()),
    })
}

// ---------------------------------------------------------------------------------------------------------------
// Sizes

/// `Number.parseInt(text, 10)` for a non-negative or signed decimal prefix; `None` for NaN.
fn parse_int_prefix(text: &str) -> Option<i64> {
    let text = jsstr::js_trim_start(text);
    let (negative, digits) = match text.as_bytes().first() {
        Some(b'-') => (true, &text[1..]),
        Some(b'+') => (false, &text[1..]),
        _ => (false, text),
    };
    let end = digits.bytes().take_while(u8::is_ascii_digit).count();
    let value: f64 = digits.get(..end).filter(|digits| !digits.is_empty())?.parse().ok()?;
    let value = if negative { -value } else { value };
    value.is_finite().then_some(value as i64)
}

/// TS: settingsInfo.ts du (`du -sk -- <file>` in KiB; `None` when unknown or timed out).
pub async fn du(file: &Path) -> Option<i64> {
    let options = ExecOptions::default().timeout(SIZE_TIMEOUT).max_buffer(DU_MAX_BUFFER);
    let file = file.to_string_lossy();
    let stdout = match proc::exec("du", &["-sk", "--", file.as_ref()], &options).await {
        Ok(output) => output.stdout,
        // du exits non-zero for unreadable children but still prints a total; a timeout prints nothing.
        Err(error) if !error.killed => error.stdout,
        Err(_) => return None,
    };
    let first = jsstr::js_trim(&stdout)
        .split(jsstr::is_js_whitespace)
        .next()
        .unwrap_or_default();
    parse_int_prefix(first)
}

/// TS: readCandidateSizes argument refusal.
fn size_paths_error() -> DhError {
    DhError::msg(format!("Pass at most {MAX_SIZE_PATHS} relative candidate paths"))
}

/// TS: settingsInfo.ts readCandidateSizes (KiB per relative path in the main checkout; symlinks 0; `Pass at most 24
/// relative candidate paths`).
pub async fn read_candidate_sizes(cwd: &str, paths: &[String]) -> Result<CandidateSizes> {
    if paths.len() > MAX_SIZE_PATHS || !paths.iter().all(|file| valid_relative_path(file)) {
        return Err(size_paths_error());
    }
    let context = resolve_repo_context(cwd).await?;
    let checkout = context.main_root.unwrap_or(context.root);
    let checkout = &checkout;
    // SIZE_CONCURRENCY workers; the record lists the paths in request order.
    let sizes: Vec<(String, Option<i64>)> = futures::stream::iter(paths.iter())
        .map(|file| async move {
            let absolute = Path::new(checkout).join(file);
            let size = match tokio::fs::symlink_metadata(&absolute).await {
                Err(_) => None,
                Ok(meta) if meta.file_type().is_symlink() => Some(0),
                Ok(meta) if meta.is_dir() => du(&absolute).await,
                Ok(meta) => Some(meta.len().div_ceil(1024) as i64),
            };
            (file.clone(), size)
        })
        .buffered(SIZE_CONCURRENCY)
        .collect()
        .await;
    Ok(sizes.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use deckhand_testkit::{Case, Outcome, replay, replay_async};
    use serde_json::json;

    #[test]
    fn classify_candidate_parity() {
        replay(
            "settingsInfo/classifyCandidate",
            |case: &Case| -> std::result::Result<Value, String> {
                let class = classify_candidate(case.arg_str(0)?);
                Ok(json!({"suggestion": class.suggestion, "reason": class.reason}))
            },
        );
    }

    #[test]
    fn codex_hook_commands_parity() {
        replay(
            "settingsInfo/codexHookCommands",
            |case: &Case| -> std::result::Result<Vec<String>, String> {
                let text = |index: usize| -> std::result::Result<Option<String>, String> {
                    if case.arg_is_undefined(index) {
                        Ok(None)
                    } else {
                        case.arg_as::<String>(index).map(Some)
                    }
                };
                let (json, toml) = (text(0)?, text(1)?);
                Ok(codex_hook_commands(json.as_deref(), toml.as_deref()))
            },
        );
    }

    #[test]
    fn codex_hook_state_parity() {
        replay(
            "settingsInfo/codexHookState",
            |case: &Case| -> std::result::Result<Value, String> {
                let commands: Vec<String> = case.arg_as(0)?;
                Ok(match codex_hook_state(&commands, case.arg_str(1)?) {
                    Some(state) => json!(state),
                    None => deckhand_testkit::tags::undefined(),
                })
            },
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn read_candidate_sizes_parity() {
        replay_async("settingsInfo/readCandidateSizes", |case: &Case| {
            let case = case.clone();
            async move {
                let cwd = match case.arg_str(0) {
                    Ok(cwd) => cwd.to_owned(),
                    Err(error) => return Outcome::throws(error),
                };
                // The wire decoder's half: anything but a list of strings is refused like an invalid path.
                let Ok(paths) = case.arg_as::<Vec<String>>(1) else {
                    return Outcome::throws(size_paths_error());
                };
                match read_candidate_sizes(&cwd, &paths).await {
                    Ok(sizes) => Outcome::ret(sizes),
                    Err(error) => Outcome::throws(error),
                }
            }
        })
        .await;
    }

    use crate::storage::tests::{Home, block_on};
    use crate::workspace_git::test_repo::{repo, temp, tgit, write};

    #[test]
    fn codex_hook_commands_follow_js_key_order_and_toml_lines() {
        let json = r#"{"b":{"command":"x"},"2":[{"command":"two"}],"1":{"command":{"command":"one"}}}"#;
        assert_eq!(codex_hook_commands(Some(json), None), ["one", "two", "x"]);
        let toml =
            "a = 1\n\n  command = \"first\"\r\ncommand='second' # c\ncommand = \"bad\nline\"\ncommand = \"\\u00e9\"";
        assert_eq!(codex_hook_commands(None, Some(toml)), ["first", "second", "é"]);
        // A stale bridge of another install is recognised as Deckhand's.
        let stale = vec!["/usr/bin/node /opt/deckhand/dist/cli.js hook".to_owned()];
        assert_eq!(
            codex_hook_state(&stale, "'/x/deckhand' 'hook'"),
            Some(CodexHookState::Other)
        );
        assert_eq!(codex_hook_state(&stale, &stale[0]), None);
        assert_eq!(codex_hook_state(&[], "x"), Some(CodexHookState::Missing));
    }

    #[test]
    fn worktree_candidates_list_configured_entries_first() {
        // The user config and state directory come from DECKHAND_HOME: hold the env lock on a temporary home.
        let home = tempfile::Builder::new().prefix("deckhand-settings-").tempdir().unwrap();
        let _home = Home::set(home.path());
        block_on(async {
            let (_dir, root) = repo();
            write(&root, ".gitignore", "node_modules\ndist\n.env\n");
            tgit(&root, &["add", ".gitignore"]);
            tgit(&root, &["commit", "-m", "ignore"]);
            std::fs::create_dir_all(Path::new(&root).join("node_modules/x")).unwrap();
            std::fs::create_dir_all(Path::new(&root).join("dist")).unwrap();
            write(&root, "dist/a.js", "a");
            write(&root, ".env", "A=1\n");
            write(&root, "notes.txt", "n");
            std::os::unix::fs::symlink(Path::new(&root).join("file.txt"), Path::new(&root).join("link")).unwrap();
            write(
                &root,
                "deckhand.json",
                r#"{"worktree":{"symlink":["node_modules","missing-dir"],"files":{".env.local":".env"}}}"#,
            );
            let read = read_worktree_candidates(&root).await.unwrap();
            let value = serde_json::to_value(&read).unwrap();
            let expected = json!({
                "checkout": root,
                "candidates": [
                    {"path": "node_modules", "kind": "dir", "ignored": true, "suggestion": "link", "reason": "dependencies", "configured": "symlink", "layers": ["repository"]},
                    {"path": "missing-dir", "kind": "file", "ignored": false, "suggestion": "skip", "reason": "", "configured": "symlink", "layers": ["repository"], "missing": true},
                    {"path": ".env.local", "kind": "file", "ignored": false, "suggestion": "link", "reason": "env file", "configured": "files", "source": ".env", "layers": ["repository"], "missing": true},
                    {"path": ".env", "kind": "file", "ignored": true, "suggestion": "link", "reason": "env file"},
                    {"path": "dist", "kind": "dir", "ignored": true, "suggestion": "skip", "reason": "build output"},
                    {"path": "link", "kind": "symlink", "target": format!("{root}/file.txt"), "ignored": false, "suggestion": "skip", "reason": ""},
                    {"path": "notes.txt", "kind": "file", "ignored": false, "suggestion": "skip", "reason": ""},
                ],
                "moreCandidates": 0,
            });
            assert_eq!(
                serde_json::to_string(&value).unwrap(),
                serde_json::to_string(&expected).unwrap()
            );

            let sizes = read_candidate_sizes(
                &root,
                &[
                    "notes.txt".to_owned(),
                    "link".to_owned(),
                    "gone".to_owned(),
                    "dist".to_owned(),
                ],
            )
            .await
            .unwrap();
            assert_eq!(sizes.keys().collect::<Vec<_>>(), ["notes.txt", "link", "gone", "dist"]);
            assert_eq!(
                (sizes["notes.txt"], sizes["link"], sizes["gone"]),
                (Some(1), Some(0), None)
            );
            assert!(sizes["dist"].is_some_and(|kib| kib >= 0));
        });
    }

    #[test]
    fn global_link_candidates_suggest_entries_shared_by_two_repositories() {
        // The user config and state directory come from DECKHAND_HOME: hold the env lock on a temporary home.
        let home = tempfile::Builder::new().prefix("deckhand-settings-").tempdir().unwrap();
        let _home = Home::set(home.path());
        block_on(async {
            let (_one, first) = repo();
            let (_two, second) = repo();
            let (_none, outside) = temp("deckhand-outside-");
            for root in [&first, &second] {
                write(root, ".gitignore", "node_modules\ndist\n.env\n");
                tgit(root, &["add", ".gitignore"]);
                tgit(root, &["commit", "-m", "ignore"]);
                std::fs::create_dir_all(Path::new(root).join("node_modules")).unwrap();
                std::fs::create_dir_all(Path::new(root).join("dist")).unwrap();
                write(root, "dist/a", "a");
                write(root, ".env", "A=1\n");
                write(root, "notes.txt", "n");
            }
            write(&first, "only-here.txt", "x");
            let known = vec![second.clone(), first.clone(), outside.clone()];
            let read = read_global_link_candidates(&first, &known).await.unwrap();
            let name = |root: &str| path::basename(root);
            let both = json!([name(&first), name(&second)]);
            let expected = json!({
                "checkout": first,
                "candidates": [
                    {"path": ".env", "kind": "file", "ignored": true, "suggestion": "link", "reason": "env file", "repos": both},
                    {"path": "node_modules", "kind": "dir", "ignored": true, "suggestion": "link", "reason": "dependencies", "repos": both},
                    {"path": "notes.txt", "kind": "file", "ignored": false, "suggestion": "skip", "reason": "", "repos": both},
                ],
                "moreCandidates": 0,
                "repos": both,
            });
            assert_eq!(serde_json::to_value(&read).unwrap(), expected);

            let none = read_global_link_candidates(&outside, &[]).await.unwrap();
            assert_eq!(
                serde_json::to_value(&none).unwrap(),
                json!({"checkout": outside, "candidates": [], "moreCandidates": 0, "repos": [], "candidatesError": "no repository could be read"})
            );
        });
    }

    // covers: B-I7-01 global link candidates: global entries with the repos that have them, then paths two or more repos share (no build output); worktrees count as their repo
    #[test]
    fn global_link_candidates_global_entries_then_shared_paths_worktrees_count_as_their_repo() {
        let home = tempfile::Builder::new()
            .prefix("deckhand-global-links-")
            .tempdir()
            .unwrap();
        let _home = Home::set(home.path());
        block_on(async {
            let (_a, a) = repo();
            let (_b, b) = repo();
            let (_c, c) = repo();
            for root in [&a, &b, &c] {
                write(root, ".gitignore", "node_modules\ndist\nlocal.cfg\nsecrets.json\n");
                tgit(root, &["add", ".gitignore"]);
                tgit(root, &["commit", "-m", "ignore"]);
            }
            for root in [&a, &b] {
                std::fs::create_dir(Path::new(root).join("node_modules")).unwrap();
                write(root, "node_modules/x", "");
                std::fs::create_dir(Path::new(root).join("dist")).unwrap();
                write(root, "dist/y", "");
                write(root, "local.cfg", "");
            }
            write(&a, "only-a.txt", "");
            write(&c, "secrets.json", "");
            let b_worktree = format!("{b}-wt");
            tgit(&b, &["worktree", "add", "-b", "side", &b_worktree]);
            crate::storage::update_app_config(|mut current| {
                current.defaults = Some(json!({"worktree": {"symlink": ["secrets.json"]}}));
                Ok(current)
            })
            .await
            .unwrap();

            let gone = home.path().join("gone").to_string_lossy().into_owned();
            let known = vec![b_worktree.clone(), c.clone(), b.clone(), gone];
            let result = read_global_link_candidates(&a, &known).await.unwrap();
            let name = |root: &str| path::basename(root);
            assert_eq!(result.repos, Some(vec![name(&a), name(&b), name(&c)]));
            assert_eq!(result.checkout, std::fs::canonicalize(&a).unwrap().to_string_lossy());
            let rows: Vec<Value> = result
                .candidates
                .iter()
                .map(|candidate| {
                    let value = serde_json::to_value(candidate).unwrap();
                    json!([
                        value["path"],
                        value.get("configured").cloned().unwrap_or(json!("-")),
                        value["kind"],
                        value["repos"]
                    ])
                })
                .collect();
            // Two repos each, the suggested link first; dist (build output) and only-a.txt (one repo) are left out.
            assert_eq!(
                rows,
                vec![
                    json!(["secrets.json", "symlink", "file", [name(&c)]]),
                    json!(["node_modules", "-", "dir", [name(&a), name(&b)]]),
                    json!(["local.cfg", "-", "file", [name(&a), name(&b)]]),
                ]
            );
            // Only this repository known: nothing is shared, and nothing fails.
            let alone = read_global_link_candidates(&a, &[]).await.unwrap();
            let paths: Vec<String> = alone
                .candidates
                .iter()
                .map(|candidate| {
                    serde_json::to_value(candidate).unwrap()["path"]
                        .as_str()
                        .unwrap()
                        .to_owned()
                })
                .collect();
            assert_eq!(
                (alone.repos, paths),
                (Some(vec![name(&a)]), vec!["secrets.json".to_owned()])
            );
            let _ = std::fs::remove_dir_all(&b_worktree);
        });
    }

    #[test]
    fn settings_info_outside_and_inside_a_repository() {
        // The user config and state directory come from DECKHAND_HOME: hold the env lock on a temporary home.
        let home = tempfile::Builder::new().prefix("deckhand-settings-").tempdir().unwrap();
        let _home = Home::set(home.path());
        block_on(async {
            let (_none, outside) = temp("deckhand-outside-");
            let info = read_settings_info(&outside).await.unwrap();
            assert_eq!(info.repository.state, RepositoryState::None);
            assert_eq!(
                (info.repository.path.as_deref(), info.repo.clone()),
                (None, path::basename(&outside))
            );
            assert!(
                info.repository
                    .error
                    .is_some_and(|error| error.starts_with("Not a Git repository: "))
            );
            assert!(!info.needs_review && !info.inside_ignored && info.vars.is_none());

            let (_dir, root) = repo();
            let info = read_settings_info(&root).await.unwrap();
            assert_eq!(info.repository.state, RepositoryState::Absent);
            assert_eq!(info.repository.path, Some(format!("{root}/deckhand.json")));
            assert_eq!(info.default_branch.as_deref(), Some("main"));
            assert_eq!(info.origin_branch, None);
            assert_eq!(info.repo, path::basename(&root));
            assert!(info.vars.is_some() && !info.inside_ignored && !info.rows.is_empty());
            assert_eq!(
                info.default_location,
                path::join(&[&home.path().to_string_lossy(), "worktrees", "<name>"])
            );

            write(&root, ".gitignore", ".worktrees/\n");
            write(&root, "deckhand.json", r#"{"defaultAgent":"pi"}"#);
            let info = read_settings_info(&root).await.unwrap();
            assert_eq!(info.repository.state, RepositoryState::Untrusted);
            assert!(info.needs_review && info.inside_ignored);
            write(&root, "deckhand.json", "{");
            let info = read_settings_info(&root).await.unwrap();
            assert_eq!(info.repository.state, RepositoryState::Invalid);
        });
    }
}
