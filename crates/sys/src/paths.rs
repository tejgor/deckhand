//! Port of paths.ts (plus the dev-home isolation of scripts/deckhand-dev.mjs).
//!
//! Every Deckhand file lives under one state directory: `DECKHAND_HOME` when set (resolved against the cwd), else
//! `~/.deckhand`. The environment is read on every call, as the TS reads `process.env` on every call, so a process
//! (or a test) that changes `DECKHAND_HOME` sees the new location at once.
//!
//! Isolation: debug builds must never touch the production home by accident. [`is_production_home`] says whether a
//! directory is (or is inside) `~/.deckhand`, comparing canonical paths the way `isolatedHome()` in
//! scripts/deckhand-dev.mjs does (symlinks and macOS `/var` vs `/private/var` resolved, missing tails kept), and
//! [`guard_debug_home`] refuses it in debug builds unless `DECKHAND_ALLOW_PROD_HOME=1`.

use std::ffi::OsStr;
use std::io;
use std::path::{Component, Path, PathBuf};

use deckhand_core::error::{DhError, Result};

/// The environment variable naming the state directory.
pub const HOME_ENV: &str = "DECKHAND_HOME";
/// The environment variable naming the channel (`dev` for the isolated dev launcher; otherwise stable).
pub const CHANNEL_ENV: &str = "DECKHAND_CHANNEL";
/// `DECKHAND_DEV=1`: running from source (TS: tsx); kept for parity of [`is_dev_runtime`].
pub const DEV_ENV: &str = "DECKHAND_DEV";
/// `DECKHAND_ALLOW_PROD_HOME=1` lets a debug build use `~/.deckhand`.
pub const ALLOW_PROD_HOME_ENV: &str = "DECKHAND_ALLOW_PROD_HOME";
/// The production state directory's name under the home directory.
pub const PRODUCTION_DIR: &str = ".deckhand";
/// deckhand-dev.mjs: the default isolated dev home's name under the home directory.
pub const DEV_HOME_DIR: &str = ".deckhand-dev";

/// Which Deckhand this process is: the stable install or the isolated dev launcher (`DECKHAND_CHANNEL=dev`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Channel {
    Stable,
    Dev,
}

impl Channel {
    /// The name the daemon reports in `ping` (`process.env.DECKHAND_CHANNEL ?? 'stable'`).
    pub fn as_str(self) -> &'static str {
        match self {
            Channel::Stable => "stable",
            Channel::Dev => "dev",
        }
    }
}

/// `process.env.DECKHAND_CHANNEL === 'dev'`.
pub fn channel() -> Channel {
    if std::env::var_os(CHANNEL_ENV).as_deref() == Some(OsStr::new("dev")) {
        Channel::Dev
    } else {
        Channel::Stable
    }
}

/// `process.env.DECKHAND_CHANNEL ?? 'stable'`, as the daemon reports it in `ping` (any other value passes through).
pub fn channel_name() -> String {
    match std::env::var_os(CHANNEL_ENV) {
        Some(value) => value.to_string_lossy().into_owned(),
        None => Channel::Stable.as_str().to_owned(),
    }
}

/// Whether this is the isolated dev channel.
pub fn is_dev_channel() -> bool {
    channel() == Channel::Dev
}

/// Node `os.homedir()`: `$HOME` when set and non-empty, else the user database entry, else `/`.
pub fn home_dir() -> PathBuf {
    match std::env::var_os("HOME") {
        Some(home) if !home.is_empty() => PathBuf::from(home),
        _ => nix::unistd::User::from_uid(nix::unistd::getuid())
            .ok()
            .flatten()
            .map(|user| user.dir)
            .unwrap_or_else(|| PathBuf::from("/")),
    }
}

/// Node `process.cwd()`; a deleted cwd falls back to `$PWD`, then `/`.
pub fn current_dir() -> PathBuf {
    std::env::current_dir()
        .ok()
        .or_else(|| std::env::var_os("PWD").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("/"))
}

/// Node `path.resolve(path)`: absolute (against the cwd) and lexically normalised (`.`/`..` folded, no trailing
/// slash). Symlinks are not resolved.
pub fn resolve(path: impl AsRef<Path>) -> PathBuf {
    resolve_from(&current_dir(), path)
}

/// Node `path.resolve(base, path)` with an absolute `base`.
pub fn resolve_from(base: &Path, path: impl AsRef<Path>) -> PathBuf {
    let path = path.as_ref();
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    };
    normalize(&joined)
}

/// Lexical normalisation of an absolute path (`..` never climbs above the root).
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::from("/");
    for component in path.components() {
        match component {
            Component::Prefix(_) | Component::RootDir | Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            Component::Normal(part) => out.push(part),
        }
    }
    out
}

/// TS: paths.ts getConfigDir (`DECKHAND_HOME` resolved, else `~/.deckhand`).
pub fn config_dir() -> PathBuf {
    config_dir_from(std::env::var_os(HOME_ENV).as_deref(), &home_dir(), &current_dir())
}

/// The pure half of [`config_dir`]: an empty `DECKHAND_HOME` counts as unset, as in TS.
pub fn config_dir_from(deckhand_home: Option<&OsStr>, home: &Path, cwd: &Path) -> PathBuf {
    match deckhand_home {
        Some(value) if !value.is_empty() => resolve_from(cwd, value),
        _ => home.join(PRODUCTION_DIR),
    }
}

/// The canonical form of a directory for comparisons: its realpath, else its resolved path (paths.ts
/// isSameConfigDir's `canonical`).
fn realpath_or_resolved(dir: &Path) -> PathBuf {
    std::fs::canonicalize(dir).unwrap_or_else(|_| resolve(dir))
}

/// TS: paths.ts isSameConfigDir (a reported Deckhand home compared with ours by realpath, e.g. macOS `/var` vs
/// `/private/var`).
pub fn is_same_config_dir(other: &Path) -> bool {
    realpath_or_resolved(other) == realpath_or_resolved(&config_dir())
}

/// TS: paths.ts getSocketPath.
pub fn socket_path() -> PathBuf {
    config_dir().join("daemon.sock")
}

/// TS: paths.ts getDaemonPidPath.
pub fn daemon_pid_path() -> PathBuf {
    config_dir().join("daemon.pid")
}

/// TS: paths.ts getDaemonLogPath.
pub fn daemon_log_path() -> PathBuf {
    config_dir().join("daemon.log")
}

/// TS: paths.ts getWorkerDir.
pub fn worker_dir() -> PathBuf {
    config_dir().join("workers")
}

/// TS: paths.ts getWorkerPidPath.
pub fn worker_pid_path(session_id: &str) -> PathBuf {
    worker_dir().join(format!("{session_id}.pid"))
}

/// TS: paths.ts getWorkerLogPath.
pub fn worker_log_path(session_id: &str) -> PathBuf {
    worker_dir().join(format!("{session_id}.log"))
}

/// TS: paths.ts getAgentSessionDir.
pub fn agent_session_dir(program: &str) -> PathBuf {
    config_dir().join("agent-sessions").join(program)
}

/// TS: paths.ts getStatePath.
pub fn state_path() -> PathBuf {
    config_dir().join("state.json")
}

/// TS: paths.ts getNotesDir (`sessions/<session id>.md`, `worktrees/<record id>.md`, `repos/<hash>.md`, `tasks/`).
pub fn notes_dir() -> PathBuf {
    config_dir().join("notes")
}

/// TS: paths.ts getConfigPath.
pub fn config_path() -> PathBuf {
    config_dir().join("config.json")
}

/// The UI preferences file (uiState.ts `ui-state.json`).
pub fn ui_state_path() -> PathBuf {
    config_dir().join("ui-state.json")
}

/// TS: paths.ts getCliEntryPath. The Rust binary is its own entry point: workers, hooks and the daemon are started
/// as `<this executable> <mode>`, so this is the running executable.
pub fn cli_entry_path() -> Result<PathBuf> {
    std::env::current_exe().map_err(DhError::from)
}

/// TS: paths.ts isDevRuntime (`DECKHAND_DEV=1`).
pub fn is_dev_runtime() -> bool {
    std::env::var_os(DEV_ENV).as_deref() == Some(OsStr::new("1"))
}

/// The production state directory, `~/.deckhand` (whatever `DECKHAND_HOME` says).
pub fn production_home() -> PathBuf {
    home_dir().join(PRODUCTION_DIR)
}

/// scripts/deckhand-dev.mjs `canonical`: the realpath of `path`; for a path that does not exist (yet), the
/// canonical form of its nearest existing ancestor with the missing tail appended, following a dangling symlink to
/// where it points. Errors other than a missing file are returned.
pub fn canonical(path: &Path) -> io::Result<PathBuf> {
    canonical_depth(&resolve(path), 0)
}

fn canonical_depth(file: &Path, depth: usize) -> io::Result<PathBuf> {
    // A symlink loop among missing paths would recurse forever in the JS too; give up like ELOOP.
    if depth > 64 {
        return Err(io::Error::from_raw_os_error(libc::ELOOP));
    }
    match std::fs::canonicalize(file) {
        Ok(real) => Ok(real),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            if let Ok(meta) = std::fs::symlink_metadata(file)
                && meta.file_type().is_symlink()
            {
                let target = std::fs::read_link(file)?;
                let parent = file.parent().unwrap_or(Path::new("/"));
                return canonical_depth(&resolve_from(parent, target), depth + 1);
            }
            match (file.parent(), file.file_name()) {
                (Some(parent), Some(name)) => Ok(canonical_depth(parent, depth + 1)?.join(name)),
                _ => Ok(file.to_path_buf()),
            }
        }
        Err(error) => Err(error),
    }
}

/// `inner` is `outer` itself or inside it (both canonical).
fn same_or_inside(inner: &Path, outer: &Path) -> bool {
    inner == outer || inner.starts_with(outer)
}

/// Whether `path` is the production state directory `~/.deckhand` or inside it, comparing canonical paths (a
/// symlink to it counts). A path that cannot be canonicalised is compared as resolved.
pub fn is_production_home(path: &Path) -> bool {
    is_production_home_in(path, &home_dir())
}

/// [`is_production_home`] with the home directory given.
pub fn is_production_home_in(path: &Path, home: &Path) -> bool {
    let canonical_or_resolved = |dir: &Path| canonical(dir).unwrap_or_else(|_| resolve(dir));
    same_or_inside(
        &canonical_or_resolved(path),
        &canonical_or_resolved(&home.join(PRODUCTION_DIR)),
    )
}

/// The message a debug build refuses the production home with.
pub fn production_home_refusal(home: &Path) -> String {
    format!(
        "Refusing to use the production Deckhand home {} from a debug build: set DECKHAND_HOME to a scratch directory, or DECKHAND_ALLOW_PROD_HOME=1 to override",
        home.display()
    )
}

/// The debug-build guard (crates/deckhand/src/guard.rs): in a debug build, an error when the resolved state directory
/// is the production home (see [`is_production_home`]) and `DECKHAND_ALLOW_PROD_HOME` is not `1`. Release builds
/// always pass.
pub fn guard_debug_home() -> Result<()> {
    if cfg!(debug_assertions) {
        let allow = std::env::var_os(ALLOW_PROD_HOME_ENV).as_deref() == Some(OsStr::new("1"));
        return guard_home(&config_dir(), &home_dir(), allow);
    }
    Ok(())
}

/// The pure half of [`guard_debug_home`] (applies regardless of the build profile).
pub fn guard_home(config_dir: &Path, home: &Path, allow_production: bool) -> Result<()> {
    if !allow_production && is_production_home_in(config_dir, home) {
        return Err(DhError::msg(production_home_refusal(config_dir)));
    }
    Ok(())
}

/// scripts/deckhand-dev.mjs isolatedHome: the canonical dev state directory (`candidate`, default
/// `$DECKHAND_DEV_HOME` or `~/.deckhand-dev`), refused when it is, contains or is inside `~/.deckhand` or (outside the
/// dev channel) `DECKHAND_HOME`.
pub fn isolated_home(candidate: Option<&Path>) -> Result<PathBuf> {
    let home = home_dir();
    let candidate = match candidate {
        Some(path) => path.to_path_buf(),
        None => match std::env::var_os("DECKHAND_DEV_HOME") {
            Some(value) if !value.is_empty() => PathBuf::from(value),
            _ => home.join(DEV_HOME_DIR),
        },
    };
    let configured = if is_dev_channel() {
        None
    } else {
        std::env::var_os(HOME_ENV)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
    };
    isolated_home_in(&candidate, &home, configured.as_deref())
}

/// The pure half of [`isolated_home`].
pub fn isolated_home_in(candidate: &Path, home: &Path, configured: Option<&Path>) -> Result<PathBuf> {
    let mut production = vec![canonical(&home.join(PRODUCTION_DIR))?];
    if let Some(configured) = configured {
        production.push(canonical(&resolve(configured))?);
    }
    let isolated = canonical(&resolve(candidate))?;
    for prod in &production {
        if same_or_inside(&isolated, prod) || prod.starts_with(&isolated) {
            return Err(DhError::msg(
                "The dev state directory must be separate from ~/.deckhand and DECKHAND_HOME (including symlinks)",
            ));
        }
    }
    Ok(isolated)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp() -> tempfile::TempDir {
        tempfile::tempdir().expect("tempdir")
    }

    #[test]
    fn config_dir_prefers_deckhand_home_resolved_against_cwd() {
        let home = Path::new("/home/me");
        let cwd = Path::new("/work/repo");
        assert_eq!(config_dir_from(None, home, cwd), PathBuf::from("/home/me/.deckhand"));
        assert_eq!(
            config_dir_from(Some(OsStr::new("")), home, cwd),
            PathBuf::from("/home/me/.deckhand")
        );
        assert_eq!(
            config_dir_from(Some(OsStr::new("state")), home, cwd),
            PathBuf::from("/work/repo/state")
        );
        assert_eq!(
            config_dir_from(Some(OsStr::new("../x/./y/")), home, cwd),
            PathBuf::from("/work/x/y")
        );
        assert_eq!(
            config_dir_from(Some(OsStr::new("/abs/dir")), home, cwd),
            PathBuf::from("/abs/dir")
        );
    }

    #[test]
    fn resolve_normalizes_like_node() {
        let base = Path::new("/a/b");
        assert_eq!(resolve_from(base, ""), PathBuf::from("/a/b"));
        assert_eq!(resolve_from(base, "."), PathBuf::from("/a/b"));
        assert_eq!(resolve_from(base, "../../../.."), PathBuf::from("/"));
        assert_eq!(resolve_from(base, "c//d/../e/"), PathBuf::from("/a/b/c/e"));
        assert_eq!(resolve_from(base, "/x/./y/.."), PathBuf::from("/x"));
    }

    #[test]
    fn file_paths_live_under_the_config_dir() {
        // Reads the real environment: only checks the shape, never touches the files.
        let dir = config_dir();
        assert_eq!(socket_path(), dir.join("daemon.sock"));
        assert_eq!(daemon_pid_path(), dir.join("daemon.pid"));
        assert_eq!(daemon_log_path(), dir.join("daemon.log"));
        assert_eq!(worker_pid_path("abc"), dir.join("workers").join("abc.pid"));
        assert_eq!(worker_log_path("abc"), dir.join("workers").join("abc.log"));
        assert_eq!(agent_session_dir("pi"), dir.join("agent-sessions").join("pi"));
        assert_eq!(state_path(), dir.join("state.json"));
        assert_eq!(notes_dir(), dir.join("notes"));
        assert_eq!(config_path(), dir.join("config.json"));
        assert_eq!(ui_state_path(), dir.join("ui-state.json"));
    }

    #[test]
    fn canonical_resolves_symlinks_and_keeps_missing_tails() {
        let root = temp();
        let real = std::fs::canonicalize(root.path()).unwrap();
        std::fs::create_dir(real.join("target")).unwrap();
        std::os::unix::fs::symlink(real.join("target"), real.join("link")).unwrap();
        assert_eq!(canonical(&root.path().join("link")).unwrap(), real.join("target"));
        assert_eq!(
            canonical(&root.path().join("link/missing/deeper")).unwrap(),
            real.join("target/missing/deeper")
        );
        // A dangling symlink is followed to where it points.
        std::os::unix::fs::symlink(real.join("nowhere"), real.join("dangling")).unwrap();
        assert_eq!(
            canonical(&root.path().join("dangling/x")).unwrap(),
            real.join("nowhere/x")
        );
    }

    #[test]
    fn production_home_is_detected_by_canonical_path() {
        let root = temp();
        let home = std::fs::canonicalize(root.path()).unwrap();
        let prod = home.join(PRODUCTION_DIR);
        // Missing production home: still detected lexically.
        assert!(is_production_home_in(&prod, &home));
        assert!(is_production_home_in(&prod.join("worktrees/x"), &home));
        std::fs::create_dir(&prod).unwrap();
        assert!(is_production_home_in(&prod, &home));
        assert!(is_production_home_in(&home.join("./.deckhand/../.deckhand"), &home));
        // A symlink to it counts; a sibling with a common prefix does not.
        std::os::unix::fs::symlink(&prod, home.join("alias")).unwrap();
        assert!(is_production_home_in(&home.join("alias"), &home));
        assert!(is_production_home_in(&home.join("alias/notes"), &home));
        assert!(!is_production_home_in(&home.join(".deckhand-dev"), &home));
        assert!(!is_production_home_in(&home.join(".deckhandx"), &home));
        assert!(!is_production_home_in(&home, &home));
        // Through a symlinked home directory.
        let linked_home = home.join("linked-home");
        std::os::unix::fs::symlink(&home, &linked_home).unwrap();
        assert!(is_production_home_in(&linked_home.join(".deckhand"), &home));
    }

    #[test]
    fn guard_refuses_production_home_unless_allowed() {
        let root = temp();
        let home = std::fs::canonicalize(root.path()).unwrap();
        let prod = home.join(PRODUCTION_DIR);
        let error = guard_home(&prod, &home, false).unwrap_err();
        assert!(
            error
                .message()
                .starts_with("Refusing to use the production Deckhand home")
        );
        assert!(error.message().contains("DECKHAND_ALLOW_PROD_HOME=1"));
        assert!(guard_home(&prod, &home, true).is_ok());
        assert!(guard_home(&home.join("scratch"), &home, false).is_ok());
    }

    #[test]
    fn isolated_home_must_be_separate_from_production() {
        let root = temp();
        let home = std::fs::canonicalize(root.path()).unwrap();
        let message =
            "The dev state directory must be separate from ~/.deckhand and DECKHAND_HOME (including symlinks)";
        assert_eq!(
            isolated_home_in(&home.join(".deckhand-dev"), &home, None).unwrap(),
            home.join(".deckhand-dev")
        );
        assert_eq!(
            isolated_home_in(&home.join(".deckhand"), &home, None)
                .unwrap_err()
                .message(),
            message
        );
        assert_eq!(
            isolated_home_in(&home.join(".deckhand/dev"), &home, None)
                .unwrap_err()
                .message(),
            message
        );
        // An ancestor of production is refused too.
        assert_eq!(isolated_home_in(&home, &home, None).unwrap_err().message(), message);
        // DECKHAND_HOME (stable channel) counts as production.
        let stable = home.join("stable");
        assert_eq!(
            isolated_home_in(&stable.join("x"), &home, Some(&stable))
                .unwrap_err()
                .message(),
            message
        );
        // Symlinks are followed.
        std::fs::create_dir(home.join(".deckhand")).unwrap();
        std::os::unix::fs::symlink(home.join(".deckhand"), home.join("sneaky")).unwrap();
        assert_eq!(
            isolated_home_in(&home.join("sneaky"), &home, None)
                .unwrap_err()
                .message(),
            message
        );
    }

    #[test]
    fn channel_names() {
        assert_eq!(Channel::Stable.as_str(), "stable");
        assert_eq!(Channel::Dev.as_str(), "dev");
    }
}
