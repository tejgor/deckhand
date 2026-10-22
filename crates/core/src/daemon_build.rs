//! The running daemon's build compared with this executable's, and what Deckhand says about it.
//!
//! A daemon outlives the binary it started from: after an update of the same protocol version the client keeps using
//! the old daemon, which keeps running the old daemon and worker code. That is compatible, so it is only mentioned
//! (a footer notice once per daemon, `deckhand doctor`) with `deckhand restart` as the way to pick the update up. A
//! daemon of another protocol is refused elsewhere (`deckhand_client::client`); its hint names the same command.
//! Restarting the daemon interrupts running sessions, which `s` resumes.

use std::cmp::Ordering;

/// The command that restarts the daemon.
pub const RESTART_COMMAND: &str = "deckhand restart";

/// How the daemon's build relates to this executable's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BuildRelation {
    /// The same version.
    Same,
    /// The daemon is an older version (the usual case after an update).
    DaemonOlder,
    /// The daemon is a newer version (an older executable talks to a daemon a newer one started).
    DaemonNewer,
    /// Different versions that do not compare as semantic versions.
    Unordered,
    /// The daemon did not say: it predates build reporting, so it is older than this executable.
    DaemonUnknown,
}

/// One dot-separated pre-release identifier: numeric ones compare numerically and before alphanumeric ones.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
enum PreIdent<'a> {
    Numeric(u64),
    Alpha(&'a str),
}

/// `MAJOR.MINOR.PATCH[-PRE][+BUILD]` (BUILD ignored, as semver precedence does).
#[derive(Debug, PartialEq, Eq)]
struct Version<'a> {
    core: [u64; 3],
    pre: Vec<PreIdent<'a>>,
}

fn parse_version(text: &str) -> Option<Version<'_>> {
    let text = text.trim().trim_start_matches('v');
    let text = text.split_once('+').map_or(text, |(version, _)| version);
    let (core_text, pre_text) = match text.split_once('-') {
        Some((core, pre)) => (core, Some(pre)),
        None => (text, None),
    };
    let mut parts = core_text.split('.');
    let mut core = [0u64; 3];
    for slot in &mut core {
        let part = parts.next()?;
        if part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        *slot = part.parse().ok()?;
    }
    if parts.next().is_some() {
        return None;
    }
    let pre = match pre_text {
        None => Vec::new(),
        Some(pre) => pre
            .split('.')
            .map(|ident| {
                if ident.is_empty() {
                    None
                } else if ident.bytes().all(|byte| byte.is_ascii_digit()) {
                    ident.parse().ok().map(PreIdent::Numeric)
                } else {
                    Some(PreIdent::Alpha(ident))
                }
            })
            .collect::<Option<Vec<_>>>()?,
    };
    Some(Version { core, pre })
}

/// Semantic-version precedence of two version strings (a leading `v` allowed); `None` when either does not parse.
pub fn compare_versions(left: &str, right: &str) -> Option<Ordering> {
    let (left, right) = (parse_version(left)?, parse_version(right)?);
    Some(left.core.cmp(&right.core).then_with(|| {
        // A pre-release comes before its release; otherwise identifiers compare in turn, fewer first.
        match (left.pre.is_empty(), right.pre.is_empty()) {
            (true, true) => Ordering::Equal,
            (true, false) => Ordering::Greater,
            (false, true) => Ordering::Less,
            (false, false) => left.pre.cmp(&right.pre),
        }
    }))
}

/// How the daemon's reported build (`None`: it reported none) relates to `client`, this executable's build.
pub fn compare_builds(client: &str, daemon: Option<&str>) -> BuildRelation {
    let Some(daemon) = daemon else {
        return BuildRelation::DaemonUnknown;
    };
    if daemon == client {
        return BuildRelation::Same;
    }
    match compare_versions(daemon, client) {
        Some(Ordering::Less) => BuildRelation::DaemonOlder,
        Some(Ordering::Greater) => BuildRelation::DaemonNewer,
        // Equal precedence but different text (build metadata) or not versions at all.
        Some(Ordering::Equal) | None => BuildRelation::Unordered,
    }
}

/// The UI's footer notice when a compatible daemon of another build is running; `None` for the same build.
pub fn update_notice(client: &str, daemon: Option<&str>) -> Option<String> {
    let text = match (compare_builds(client, daemon), daemon) {
        (BuildRelation::Same, _) => return None,
        (BuildRelation::DaemonOlder, Some(daemon)) => {
            format!("Daemon {daemon} is still running (this is {client}) · restart when convenient: {RESTART_COMMAND}")
        }
        (BuildRelation::DaemonNewer, Some(daemon)) => format!(
            "Daemon {daemon} is newer than this deckhand ({client}) · update this install, or {RESTART_COMMAND} to run {client}"
        ),
        (BuildRelation::Unordered, Some(daemon)) => {
            format!("Daemon {daemon} is running (this is {client}) · restart when convenient: {RESTART_COMMAND}")
        }
        _ => {
            format!("An older daemon is still running (this is {client}) · restart when convenient: {RESTART_COMMAND}")
        }
    };
    Some(text)
}

/// The identity a notice is shown once for: the daemon process and both builds. A new daemon (another pid), another
/// daemon build or another executable build each get the notice again.
pub fn notice_key(daemon_pid: Option<u32>, daemon: Option<&str>, client: &str) -> String {
    let pid = daemon_pid.map_or_else(|| "?".to_owned(), |pid| pid.to_string());
    format!("pid={pid} daemon={} client={client}", daemon.unwrap_or("?"))
}

/// `deckhand doctor`'s line about the running daemon (`daemon_pid` as reported, else from `daemon.pid`).
pub fn doctor_line(client: &str, daemon: Option<&str>, daemon_pid: Option<u32>) -> String {
    let pid = daemon_pid.map_or_else(String::new, |pid| format!(" (pid {pid})"));
    let shown = daemon.unwrap_or("an older build");
    match compare_builds(client, daemon) {
        BuildRelation::Same => format!("✓ Daemon: {shown}{pid}, the same build as this deckhand\n"),
        _ => format!(
            "• Daemon: {shown}{pid} is running; this deckhand is {client}. It stays compatible; restart when \
             convenient to run {client}: {RESTART_COMMAND} (running sessions are interrupted; resume them with s)\n"
        ),
    }
}

/// `deckhand restart`'s question before interrupting `running` sessions.
/// (`None`: the daemon could not list them; the prompt adds ` [y/N] `).
pub fn restart_prompt(running: Option<usize>) -> String {
    match running {
        Some(running) => format!(
            "Restart the Deckhand daemon? {running} running session(s) will be interrupted (resume them with s)."
        ),
        None => {
            "Restart the Deckhand daemon? Its running sessions will be interrupted (resume them with s).".to_owned()
        }
    }
}

/// `deckhand restart`'s refusal without a terminal to ask on and without `--yes`.
pub fn restart_refusal(running: Option<usize>) -> String {
    let sessions = match running {
        Some(running) => format!("{running} running session(s) would be interrupted"),
        None => "its running sessions would be interrupted".to_owned(),
    };
    format!(
        "Not restarting the Deckhand daemon: {sessions} (resume them with s). Run `{RESTART_COMMAND} --yes` to \
         restart it anyway."
    )
}

/// `deckhand restart`'s closing line: the build that was running (`None`: unknown) and the one now running.
pub fn restart_summary(old: Option<&str>, new: Option<&str>) -> String {
    match (old, new) {
        (Some(old), Some(new)) if old == new => format!("Daemon restarted ({new})."),
        (Some(old), Some(new)) => format!("Daemon restarted ({old} → {new})."),
        (None, Some(new)) => format!("Daemon restarted (now {new})."),
        (_, None) => "Daemon restarted.".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_by_semver_precedence() {
        assert_eq!(compare_versions("3.0.0", "3.0.1"), Some(Ordering::Less));
        assert_eq!(compare_versions("3.10.0", "3.9.9"), Some(Ordering::Greater));
        assert_eq!(compare_versions("v3.0.1", "3.0.1"), Some(Ordering::Equal));
        // A pre-release precedes its release; numeric identifiers compare as numbers, before alphanumeric ones.
        assert_eq!(compare_versions("3.1.0-rc.1", "3.1.0"), Some(Ordering::Less));
        assert_eq!(compare_versions("3.1.0-rc.10", "3.1.0-rc.9"), Some(Ordering::Greater));
        assert_eq!(compare_versions("3.1.0-1", "3.1.0-alpha"), Some(Ordering::Less));
        assert_eq!(compare_versions("3.1.0-rc", "3.1.0-rc.1"), Some(Ordering::Less));
        // Build metadata does not count.
        assert_eq!(compare_versions("3.0.1+abc", "3.0.1+def"), Some(Ordering::Equal));
        for bad in ["3.0", "3.0.0.0", "x.y.z", "", "3..1", "3.0.1-", "3.0.1-rc..1"] {
            assert_eq!(compare_versions(bad, "3.0.0"), None, "{bad}");
        }
    }

    #[test]
    fn builds_are_classified_against_this_executable() {
        assert_eq!(compare_builds("3.0.1", Some("3.0.1")), BuildRelation::Same);
        assert_eq!(compare_builds("3.0.1", Some("3.0.0")), BuildRelation::DaemonOlder);
        assert_eq!(compare_builds("3.0.1", Some("3.1.0")), BuildRelation::DaemonNewer);
        assert_eq!(compare_builds("3.0.1", Some("3.0.1+dirty")), BuildRelation::Unordered);
        assert_eq!(compare_builds("3.0.1", Some("dev")), BuildRelation::Unordered);
        assert_eq!(compare_builds("3.0.1", None), BuildRelation::DaemonUnknown);
    }

    #[test]
    fn the_notice_names_both_builds_and_the_restart_command() {
        assert_eq!(update_notice("3.0.1", Some("3.0.1")), None);
        assert_eq!(
            update_notice("3.0.1", Some("3.0.0")).as_deref(),
            Some("Daemon 3.0.0 is still running (this is 3.0.1) · restart when convenient: deckhand restart")
        );
        assert_eq!(
            update_notice("3.0.1", Some("3.0.2")).as_deref(),
            Some(
                "Daemon 3.0.2 is newer than this deckhand (3.0.1) · update this install, or deckhand restart to run 3.0.1"
            )
        );
        assert_eq!(
            update_notice("3.0.1", Some("dev")).as_deref(),
            Some("Daemon dev is running (this is 3.0.1) · restart when convenient: deckhand restart")
        );
        assert_eq!(
            update_notice("3.0.1", None).as_deref(),
            Some("An older daemon is still running (this is 3.0.1) · restart when convenient: deckhand restart")
        );
    }

    #[test]
    fn the_notice_key_changes_with_the_daemon_process_and_either_build() {
        let key = notice_key(Some(42), Some("3.0.0"), "3.0.1");
        assert_eq!(key, "pid=42 daemon=3.0.0 client=3.0.1");
        assert_ne!(key, notice_key(Some(43), Some("3.0.0"), "3.0.1"));
        assert_ne!(key, notice_key(Some(42), Some("3.0.1"), "3.0.2"));
        assert_ne!(key, notice_key(Some(42), Some("3.0.0"), "3.0.2"));
        assert_eq!(notice_key(None, None, "3.0.1"), "pid=? daemon=? client=3.0.1");
    }

    #[test]
    fn doctor_reports_the_daemon_build() {
        assert_eq!(
            doctor_line("3.0.1", Some("3.0.1"), Some(7)),
            "✓ Daemon: 3.0.1 (pid 7), the same build as this deckhand\n"
        );
        let older = doctor_line("3.0.1", Some("3.0.0"), Some(7));
        assert!(
            older.starts_with("• Daemon: 3.0.0 (pid 7) is running; this deckhand is 3.0.1."),
            "{older}"
        );
        assert!(older.contains("deckhand restart"), "{older}");
        let unknown = doctor_line("3.0.1", None, None);
        assert!(unknown.starts_with("• Daemon: an older build is running;"), "{unknown}");
    }

    #[test]
    fn restart_texts() {
        assert_eq!(
            restart_prompt(Some(2)),
            "Restart the Deckhand daemon? 2 running session(s) will be interrupted (resume them with s)."
        );
        assert!(restart_prompt(None).contains("Its running sessions will be interrupted"));
        assert_eq!(
            restart_refusal(Some(1)),
            "Not restarting the Deckhand daemon: 1 running session(s) would be interrupted (resume them with s). \
             Run `deckhand restart --yes` to restart it anyway."
        );
        assert_eq!(
            restart_summary(Some("3.0.0"), Some("3.0.1")),
            "Daemon restarted (3.0.0 → 3.0.1)."
        );
        assert_eq!(
            restart_summary(Some("3.0.1"), Some("3.0.1")),
            "Daemon restarted (3.0.1)."
        );
        assert_eq!(restart_summary(None, Some("3.0.1")), "Daemon restarted (now 3.0.1).");
        assert_eq!(restart_summary(Some("3.0.0"), None), "Daemon restarted.");
    }
}
