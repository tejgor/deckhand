//! Deckhand's own updates: what the latest release is, how this executable was installed, and the command that
//! updates it.
//!
//! The daemon looks the latest release up about once a day (`deckhand_sys::self_update_check`) by asking GitHub where
//! `releases/latest` redirects, and keeps the answer in the state directory's `update-check` file. The UI's header and
//! the Agents screen read that file; `deckhand update` looks again and runs the upgrade for the way Deckhand was
//! installed. Everything here is pure: the I/O (reading receipts, resolving paths, running commands) is the caller's.

use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::daemon_build::compare_versions;

/// The repository releases come from.
pub const REPO_URL: &str = "https://github.com/tejgor/deckhand";
/// Where GitHub redirects to the latest release's page (`…/releases/tag/vX.Y.Z`).
pub const LATEST_RELEASE_URL: &str = "https://github.com/tejgor/deckhand/releases/latest";
/// The releases page, for installs `deckhand update` cannot update.
pub const RELEASES_URL: &str = "https://github.com/tejgor/deckhand/releases";
/// The Homebrew formula, by its tap.
pub const HOMEBREW_FORMULA: &str = "tejgor/tap/deckhand";
/// The command that updates Deckhand.
pub const UPDATE_COMMAND: &str = "deckhand update";
/// How long a looked-up latest release stays current: the daemon looks again after this.
pub const CHECK_MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);
/// How long after the daemon starts it first looks (not at once: a daemon restarted in a loop never asks).
pub const FIRST_CHECK_DELAY: Duration = Duration::from_secs(60);
/// How often the daemon asks whether its last look is older than [`CHECK_MAX_AGE`].
pub const CHECK_TICK: Duration = Duration::from_secs(60 * 60);

/// The `update-check` file: the latest release found and when (an ISO time).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateCheck {
    pub latest: String,
    pub checked_at: String,
}

/// The version a `releases/latest` redirect names (`…/releases/tag/v3.4.0` → `3.4.0`); `None` for anything else, a
/// pre-release included.
pub fn version_from_redirect(location: &str) -> Option<String> {
    let location = location.trim();
    let tag = location.strip_prefix(REPO_URL)?.strip_prefix("/releases/tag/")?;
    let version = tag.strip_prefix('v').unwrap_or(tag);
    // Only `MAJOR.MINOR.PATCH`: GitHub never sends `latest` to a pre-release, so anything else is not an answer.
    let plain = version.split('.').count() == 3 && version.split('.').all(|part| part.parse::<u64>().is_ok());
    (plain && compare_versions(version, version).is_some()).then(|| version.to_owned())
}

/// `latest` when it is a newer release than `current`.
pub fn newer_release<'a>(current: &str, latest: Option<&'a str>) -> Option<&'a str> {
    latest.filter(|latest| compare_versions(latest, current) == Some(std::cmp::Ordering::Greater))
}

/// The header's update hint, one for every update: Deckhand's own (`deckhand_new`: the newer release) and the agents
/// with a newer release, by name. `deckhand 3.3.0 → 3.4.0 · G`, `codex update · G`, `2 agent updates · G`,
/// `deckhand → 3.4.0 + codex · G`, `deckhand → 3.4.0 + 2 agents · G`; empty when there is none.
pub fn update_hint(current: &str, deckhand_new: Option<&str>, agents: &[&str]) -> String {
    match (deckhand_new, agents) {
        (None, []) => String::new(),
        (None, [agent]) => format!("{agent} update · G"),
        (None, agents) => format!("{} agent updates · G", agents.len()),
        (Some(new), []) => format!("deckhand {current} → {new} · G"),
        (Some(new), [agent]) => format!("deckhand → {new} + {agent} · G"),
        (Some(new), agents) => format!("deckhand → {new} + {} agents · G", agents.len()),
    }
}

/// [`update_hint`] for a narrow header: the Deckhand part without the version it replaces.
pub fn short_update_hint(deckhand_new: Option<&str>, agents: &[&str]) -> String {
    match (deckhand_new, agents) {
        (Some(new), []) => format!("deckhand → {new} · G"),
        _ => update_hint("", deckhand_new, agents),
    }
}

/// The Agents screen's line about Deckhand itself (`latest`: the release found, if any).
pub fn agents_screen_line(current: &str, latest: Option<&str>) -> String {
    match (newer_release(current, latest), latest) {
        (Some(new), _) => format!("Deckhand {current} → {new} available · run {UPDATE_COMMAND}"),
        (None, Some(_)) => format!("Deckhand {current} · the latest release"),
        (None, None) => format!("Deckhand {current}"),
    }
}

/// How this executable was installed, so `deckhand update` knows how to update it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InstallMethod {
    /// A debug build or the dev channel: updated from its checkout.
    Dev,
    /// `brew install tejgor/tap/deckhand`: `prefix` is Homebrew's (`/opt/homebrew`), where `bin/brew` and the
    /// version-independent `bin/deckhand` are.
    Homebrew { prefix: PathBuf },
    /// The release's installer script, into `dir` (its receipt names it).
    Installer { dir: PathBuf },
    /// `cargo install --git <url>`, into `bin`.
    CargoGit { url: String, bin: PathBuf },
    /// `cargo install --path <path>` (a checkout).
    CargoPath { path: String },
    /// Two methods claim this executable (`labels`); the user knows which they used.
    Ambiguous { path: PathBuf, labels: Vec<&'static str> },
    /// Anything else, a release archive included.
    Unknown { path: PathBuf },
}

impl InstallMethod {
    /// How it was installed, for `Installed with …`.
    pub fn label(&self) -> &'static str {
        match self {
            InstallMethod::Dev => "a development build",
            InstallMethod::Homebrew { .. } => "Homebrew",
            InstallMethod::Installer { .. } => "the installer script",
            InstallMethod::CargoGit { .. } => "cargo install --git",
            InstallMethod::CargoPath { .. } => "cargo install --path",
            InstallMethod::Ambiguous { .. } | InstallMethod::Unknown { .. } => "an unknown method",
        }
    }
}

/// What `deckhand update` knows about this executable, read by the caller.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InstallFacts {
    /// The executable's realpath.
    pub canonical: PathBuf,
    /// A debug build or the dev channel.
    pub dev: bool,
    /// The source of cargo's install of `deckhand` (`.crates2.json`), when that install is this executable, with
    /// cargo's `bin` directory.
    pub cargo: Option<(String, PathBuf)>,
    /// The installer receipt's directory, when the binary it installed is this executable.
    pub installer_dir: Option<PathBuf>,
}

/// The Homebrew prefix of `canonical` when it is a formula's binary: `<prefix>/Cellar/deckhand/<version>/bin/deckhand`.
pub fn homebrew_prefix(canonical: &Path) -> Option<PathBuf> {
    let parts: Vec<Component<'_>> = canonical.components().collect();
    let cellar = parts.iter().rposition(|part| part.as_os_str() == "Cellar")?;
    let tail: Vec<&std::ffi::OsStr> = parts.get(cellar + 1..)?.iter().map(|part| part.as_os_str()).collect();
    match tail.as_slice() {
        [formula, _version, bin, name] if *formula == "deckhand" && *bin == "bin" && *name == "deckhand" => {
            Some(parts[..cellar].iter().collect())
        }
        _ => None,
    }
}

/// Homebrew's version-independent link to the formula's binary (`<prefix>/opt/deckhand/bin/deckhand`) for `exe` in
/// its Cellar. A link to the Cellar path breaks once an upgrade removes that version; this one follows upgrades.
pub fn homebrew_stable_path(exe: &Path) -> Option<PathBuf> {
    homebrew_prefix(exe).map(|prefix| prefix.join("opt").join("deckhand").join("bin").join("deckhand"))
}

/// The source cargo recorded for its `deckhand` install, from `.crates2.json` (`git+https://…#sha`,
/// `path+file:///…`, `registry+…`).
pub fn cargo_source(crates2: &str) -> Option<String> {
    let doc: Value = serde_json::from_str(crates2).ok()?;
    doc.get("installs")?.as_object()?.iter().find_map(|(key, install)| {
        let (name, rest) = key.split_once(' ')?;
        let installs_deckhand = install
            .get("bins")
            .and_then(Value::as_array)
            .is_some_and(|bins| bins.iter().any(|bin| bin.as_str() == Some("deckhand")));
        if name != "deckhand" || !installs_deckhand {
            return None;
        }
        let (_version, source) = rest.split_once(' ')?;
        Some(source.trim_start_matches('(').trim_end_matches(')').to_owned())
    })
}

/// The binaries an installer receipt says it put in place: `<install_prefix>/deckhand`, and `<install_prefix>/bin/deckhand`
/// for its layouts with a `bin` directory.
pub fn receipt_binaries(receipt: &str) -> Vec<PathBuf> {
    let Ok(doc) = serde_json::from_str::<Value>(receipt) else {
        return Vec::new();
    };
    let ours = doc.pointer("/source/app_name").and_then(Value::as_str) == Some("deckhand");
    let Some(prefix) = doc.get("install_prefix").and_then(Value::as_str).filter(|_| ours) else {
        return Vec::new();
    };
    let prefix = Path::new(prefix);
    vec![prefix.join("deckhand"), prefix.join("bin").join("deckhand")]
}

/// How the executable `facts` describe was installed.
pub fn install_method(facts: &InstallFacts) -> InstallMethod {
    if facts.dev {
        return InstallMethod::Dev;
    }
    if let Some(prefix) = homebrew_prefix(&facts.canonical) {
        return InstallMethod::Homebrew { prefix };
    }
    let cargo = facts.cargo.as_ref().and_then(|(source, bin)| {
        if let Some(path) = source.strip_prefix("path+file://") {
            Some(InstallMethod::CargoPath { path: path.to_owned() })
        } else {
            source.strip_prefix("git+").map(|git| InstallMethod::CargoGit {
                url: git.split(['?', '#']).next().unwrap_or(git).to_owned(),
                bin: bin.clone(),
            })
        }
    });
    let installer = facts.installer_dir.clone().map(|dir| InstallMethod::Installer { dir });
    match (cargo, installer) {
        (Some(cargo), Some(installer)) => InstallMethod::Ambiguous {
            path: facts.canonical.clone(),
            labels: vec![installer.label(), cargo.label()],
        },
        (Some(method), None) | (None, Some(method)) => method,
        (None, None) => InstallMethod::Unknown {
            path: facts.canonical.clone(),
        },
    }
}

/// The installer script of release `version`.
pub fn installer_url(version: &str) -> String {
    format!("{RELEASES_URL}/download/v{version}/deckhand-installer.sh")
}

/// The command `deckhand update` runs for one install method.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UpgradeCommand {
    pub program: String,
    pub args: Vec<String>,
    /// Added to the environment.
    pub env: Vec<(String, String)>,
    /// What to show the user (and what they could run themselves).
    pub display: String,
    /// The `deckhand` the upgrade leaves, at a path that does not change with the version.
    pub binary: PathBuf,
}

/// The shell lines the installer method runs: fetch the script to a file (`$2`) first, so a failed download fails
/// the update instead of running an empty script, then run it.
/// Downloads the installer script (`$1`) into a file `mktemp` names, so another user can't plant a link at a
/// predictable path in a shared temporary directory, runs it, and removes it.
const INSTALLER_SCRIPT: &str = r#"script=$(mktemp "${TMPDIR:-/tmp}/deckhand-installer.XXXXXX") || exit 1; curl --proto '=https' --tlsv1.2 -LsSf "$1" -o "$script" && sh "$script"; status=$?; rm -f "$script"; exit $status"#;

/// The command that updates `method` to release `version`; `None` for the methods `deckhand update` leaves to the
/// user.
pub fn upgrade_command(method: &InstallMethod, version: &str) -> Option<UpgradeCommand> {
    match method {
        InstallMethod::Homebrew { prefix } => {
            let brew = prefix.join("bin").join("brew");
            Some(UpgradeCommand {
                display: format!("brew upgrade {HOMEBREW_FORMULA}"),
                program: brew.to_string_lossy().into_owned(),
                args: vec!["upgrade".to_owned(), HOMEBREW_FORMULA.to_owned()],
                env: Vec::new(),
                binary: prefix.join("bin").join("deckhand"),
            })
        }
        InstallMethod::Installer { dir } => {
            let url = installer_url(version);
            let dir_text = dir.to_string_lossy().into_owned();
            Some(UpgradeCommand {
                display: format!(
                    "curl --proto '=https' --tlsv1.2 -LsSf {url} | DECKHAND_INSTALL_DIR={} DECKHAND_NO_MODIFY_PATH=1 sh",
                    shell_quote(&dir_text)
                ),
                program: "sh".to_owned(),
                args: vec!["-c".to_owned(), INSTALLER_SCRIPT.to_owned(), "sh".to_owned(), url],
                env: vec![
                    ("DECKHAND_INSTALL_DIR".to_owned(), dir_text),
                    ("DECKHAND_NO_MODIFY_PATH".to_owned(), "1".to_owned()),
                ],
                binary: dir.join("deckhand"),
            })
        }
        InstallMethod::CargoGit { url, bin } => {
            let args: Vec<String> = [
                "install",
                "--git",
                url,
                "--tag",
                &format!("v{version}"),
                "--locked",
                "--force",
                "deckhand",
            ]
            .iter()
            .map(|arg| (*arg).to_owned())
            .collect();
            Some(UpgradeCommand {
                display: format!("cargo {}", args.join(" ")),
                program: "cargo".to_owned(),
                args,
                env: Vec::new(),
                binary: bin.join("deckhand"),
            })
        }
        InstallMethod::Dev
        | InstallMethod::CargoPath { .. }
        | InstallMethod::Ambiguous { .. }
        | InstallMethod::Unknown { .. } => None,
    }
}

/// What to do instead, for the methods `deckhand update` does not run (`version`: the release to update to).
pub fn manual_instructions(method: &InstallMethod, version: &str) -> String {
    match method {
        InstallMethod::Dev => {
            "This is a development build: update it from its checkout (docs/dev-build.md).".to_owned()
        }
        InstallMethod::CargoPath { path } => {
            let checkout = Path::new(path)
                .parent()
                .and_then(Path::parent)
                .map_or_else(|| path.clone(), |root| root.to_string_lossy().into_owned());
            format!(
                "Installed from the checkout at {checkout}. To update it there:\n  \
                 git switch main && git pull\n  \
                 cargo install --path crates/deckhand --locked\n\
                 (docs/dev-build.md, \"Updating your own Deckhand from this checkout\")"
            )
        }
        InstallMethod::Ambiguous { path, labels } => format!(
            "{} could have been installed with {}. Update it the way you installed it; the releases are at \
             {RELEASES_URL}/tag/v{version}",
            path.display(),
            labels.join(" or ")
        ),
        InstallMethod::Unknown { path } => format!(
            "Deckhand doesn't know how {} was installed (a release archive?). Download {version} from \
             {RELEASES_URL}/tag/v{version}, or see the Updating section of docs/install.md.",
            path.display()
        ),
        InstallMethod::Homebrew { .. } | InstallMethod::Installer { .. } | InstallMethod::CargoGit { .. } => {
            String::new()
        }
    }
}

/// A word for a POSIX shell: as it is when it is plainly safe, else single-quoted.
fn shell_quote(word: &str) -> String {
    let plain = !word.is_empty()
        && word
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"/._-~+=:,@".contains(&byte));
    if plain {
        word.to_owned()
    } else {
        format!("'{}'", word.replace('\'', r"'\''"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_latest_release_is_read_from_the_redirect() {
        let at = |tag: &str| version_from_redirect(&format!("{REPO_URL}/releases/tag/{tag}"));
        assert_eq!(at("v3.4.0").as_deref(), Some("3.4.0"));
        assert_eq!(at("3.10.2").as_deref(), Some("3.10.2"));
        assert_eq!(
            version_from_redirect(&format!("{REPO_URL}/releases/tag/v3.4.0\n")).as_deref(),
            Some("3.4.0")
        );
        // No redirect (curl prints nothing), another page, another repository, a pre-release or junk.
        for location in [
            "",
            RELEASES_URL,
            "https://github.com/someone/else/releases/tag/v3.4.0",
            &format!("{REPO_URL}/releases/tag/v3.4.0-rc.1"),
            &format!("{REPO_URL}/releases/tag/v3.4"),
            &format!("{REPO_URL}/releases/tag/latest"),
        ] {
            assert_eq!(version_from_redirect(location), None, "{location}");
        }
    }

    #[test]
    fn only_a_newer_release_counts() {
        assert_eq!(newer_release("3.3.0", Some("3.4.0")), Some("3.4.0"));
        assert_eq!(newer_release("3.3.0", Some("3.3.0")), None);
        assert_eq!(newer_release("3.4.0", Some("3.3.0")), None);
        assert_eq!(newer_release("3.4.0-rc.1", Some("3.4.0")), Some("3.4.0"));
        assert_eq!(newer_release("dev", Some("3.4.0")), None);
        assert_eq!(newer_release("3.3.0", None), None);
    }

    #[test]
    fn the_header_shows_one_hint_for_every_update() {
        assert_eq!(update_hint("3.3.0", None, &[]), "");
        assert_eq!(update_hint("3.3.0", None, &["codex"]), "codex update · G");
        assert_eq!(update_hint("3.3.0", None, &["claude", "codex"]), "2 agent updates · G");
        assert_eq!(update_hint("3.3.0", Some("3.4.0"), &[]), "deckhand 3.3.0 → 3.4.0 · G");
        assert_eq!(
            update_hint("3.3.0", Some("3.4.0"), &["codex"]),
            "deckhand → 3.4.0 + codex · G"
        );
        assert_eq!(
            update_hint("3.3.0", Some("3.4.0"), &["claude", "codex"]),
            "deckhand → 3.4.0 + 2 agents · G"
        );
        assert_eq!(short_update_hint(Some("3.4.0"), &[]), "deckhand → 3.4.0 · G");
        assert_eq!(short_update_hint(None, &["pi"]), "pi update · G");
    }

    #[test]
    fn the_agents_screen_says_where_deckhand_stands() {
        assert_eq!(
            agents_screen_line("3.3.0", Some("3.4.0")),
            "Deckhand 3.3.0 → 3.4.0 available · run deckhand update"
        );
        assert_eq!(
            agents_screen_line("3.4.0", Some("3.4.0")),
            "Deckhand 3.4.0 · the latest release"
        );
        assert_eq!(agents_screen_line("3.4.0", None), "Deckhand 3.4.0");
    }

    fn facts(canonical: &str) -> InstallFacts {
        InstallFacts {
            canonical: PathBuf::from(canonical),
            ..InstallFacts::default()
        }
    }

    #[test]
    fn homebrew_installs_are_known_by_their_cellar_path() {
        for (path, prefix) in [
            ("/opt/homebrew/Cellar/deckhand/3.3.0/bin/deckhand", "/opt/homebrew"),
            ("/usr/local/Cellar/deckhand/3.3.0_1/bin/deckhand", "/usr/local"),
            (
                "/home/linuxbrew/.linuxbrew/Cellar/deckhand/3.3.0/bin/deckhand",
                "/home/linuxbrew/.linuxbrew",
            ),
        ] {
            assert_eq!(
                install_method(&facts(path)),
                InstallMethod::Homebrew {
                    prefix: PathBuf::from(prefix)
                },
                "{path}"
            );
        }
        assert_eq!(
            homebrew_stable_path(Path::new(
                "/home/linuxbrew/.linuxbrew/Cellar/deckhand/3.3.0/bin/deckhand"
            )),
            Some(PathBuf::from("/home/linuxbrew/.linuxbrew/opt/deckhand/bin/deckhand"))
        );
        // Another formula's Cellar, or a binary that is only near one, is not ours.
        for path in [
            "/opt/homebrew/Cellar/other/1.0/bin/deckhand",
            "/opt/homebrew/Cellar/deckhand/3.3.0/libexec/deckhand",
            "/opt/homebrew/bin/deckhand",
        ] {
            assert_eq!(homebrew_prefix(Path::new(path)), None, "{path}");
        }
    }

    #[test]
    fn cargo_records_where_its_install_came_from() {
        let crates2 = r#"{"installs":{
            "ripgrep 14.1.0 (registry+https://github.com/rust-lang/crates.io-index)":{"bins":["rg"]},
            "deckhand 3.3.0 (git+https://github.com/tejgor/deckhand?tag=v3.3.0#0123abc)":{"bins":["deckhand"]}
        }}"#;
        let source = cargo_source(crates2);
        assert_eq!(
            source.as_deref(),
            Some("git+https://github.com/tejgor/deckhand?tag=v3.3.0#0123abc")
        );
        let bin = PathBuf::from("/h/.cargo/bin");
        let git = InstallFacts {
            cargo: Some((source.unwrap(), bin.clone())),
            ..facts("/h/.cargo/bin/deckhand")
        };
        assert_eq!(
            install_method(&git),
            InstallMethod::CargoGit {
                url: REPO_URL.to_owned(),
                bin
            }
        );
        let path =
            r#"{"installs":{"deckhand 3.2.0 (path+file:///src/deckhand/crates/deckhand)":{"bins":["deckhand"]}}}"#;
        let checkout = InstallFacts {
            cargo: Some((cargo_source(path).unwrap(), PathBuf::from("/h/.cargo/bin"))),
            ..facts("/h/.cargo/bin/deckhand")
        };
        assert_eq!(
            install_method(&checkout),
            InstallMethod::CargoPath {
                path: "/src/deckhand/crates/deckhand".to_owned()
            }
        );
        assert!(manual_instructions(&install_method(&checkout), "3.4.0").contains("checkout at /src/deckhand."));
        for other in [
            "",
            "{}",
            r#"{"installs":{"other 1.0.0 (path+file:///x)":{"bins":["deckhand"]}}}"#,
        ] {
            assert_eq!(cargo_source(other), None, "{other}");
        }
    }

    #[test]
    fn the_installer_receipt_names_its_directory() {
        let receipt = r#"{"binaries":["deckhand"],"install_layout":"flat","install_prefix":"/h/.local/bin",
            "source":{"app_name":"deckhand","name":"deckhand","owner":"tejgor","release_type":"github"},"version":"3.3.0"}"#;
        assert_eq!(
            receipt_binaries(receipt),
            vec![
                PathBuf::from("/h/.local/bin/deckhand"),
                PathBuf::from("/h/.local/bin/bin/deckhand")
            ]
        );
        assert!(receipt_binaries(r#"{"install_prefix":"/x","source":{"app_name":"other"}}"#).is_empty());
        assert!(receipt_binaries("not json").is_empty());
        let installer = InstallFacts {
            installer_dir: Some(PathBuf::from("/h/.local/bin")),
            ..facts("/h/.local/bin/deckhand")
        };
        assert_eq!(
            install_method(&installer),
            InstallMethod::Installer {
                dir: PathBuf::from("/h/.local/bin")
            }
        );
    }

    #[test]
    fn unclear_installs_are_left_to_the_user() {
        assert_eq!(
            install_method(&facts("/h/.local/bin/deckhand")),
            InstallMethod::Unknown {
                path: PathBuf::from("/h/.local/bin/deckhand")
            }
        );
        let both = InstallFacts {
            cargo: Some((
                "git+https://github.com/tejgor/deckhand#abc".to_owned(),
                PathBuf::from("/h/bin"),
            )),
            installer_dir: Some(PathBuf::from("/h/bin")),
            ..facts("/h/bin/deckhand")
        };
        let method = install_method(&both);
        assert_eq!(
            method,
            InstallMethod::Ambiguous {
                path: PathBuf::from("/h/bin/deckhand"),
                labels: vec!["the installer script", "cargo install --git"]
            }
        );
        assert!(manual_instructions(&method, "3.4.0").contains("the installer script or cargo install --git"));
        // A development build is never updated in place, wherever it is.
        let dev = InstallFacts {
            dev: true,
            ..facts("/opt/homebrew/Cellar/deckhand/3.3.0/bin/deckhand")
        };
        assert_eq!(install_method(&dev), InstallMethod::Dev);
    }

    #[test]
    fn each_method_has_its_upgrade_command() {
        let brew = upgrade_command(
            &InstallMethod::Homebrew {
                prefix: PathBuf::from("/opt/homebrew"),
            },
            "3.4.0",
        )
        .unwrap();
        assert_eq!(brew.program, "/opt/homebrew/bin/brew");
        assert_eq!(brew.args, ["upgrade", "tejgor/tap/deckhand"]);
        assert_eq!(brew.display, "brew upgrade tejgor/tap/deckhand");
        assert_eq!(brew.binary, PathBuf::from("/opt/homebrew/bin/deckhand"));

        let installer = upgrade_command(
            &InstallMethod::Installer {
                dir: PathBuf::from("/h/my bin"),
            },
            "3.4.0",
        )
        .unwrap();
        let url = "https://github.com/tejgor/deckhand/releases/download/v3.4.0/deckhand-installer.sh";
        assert_eq!(installer.program, "sh");
        assert_eq!(installer.args[3..], [url]);
        // The script goes into a file mktemp names, never a predictable path.
        assert!(installer.args[1].contains("mktemp") && !installer.args[1].contains("$2"));
        assert_eq!(
            installer.env,
            [
                ("DECKHAND_INSTALL_DIR".to_owned(), "/h/my bin".to_owned()),
                ("DECKHAND_NO_MODIFY_PATH".to_owned(), "1".to_owned())
            ]
        );
        assert_eq!(
            installer.display,
            format!(
                "curl --proto '=https' --tlsv1.2 -LsSf {url} | DECKHAND_INSTALL_DIR='/h/my bin' DECKHAND_NO_MODIFY_PATH=1 sh"
            )
        );
        assert_eq!(installer.binary, PathBuf::from("/h/my bin/deckhand"));

        let cargo = upgrade_command(
            &InstallMethod::CargoGit {
                url: REPO_URL.to_owned(),
                bin: PathBuf::from("/h/.cargo/bin"),
            },
            "3.4.0",
        )
        .unwrap();
        assert_eq!(
            cargo.display,
            "cargo install --git https://github.com/tejgor/deckhand --tag v3.4.0 --locked --force deckhand"
        );
        assert_eq!(cargo.binary, PathBuf::from("/h/.cargo/bin/deckhand"));

        for left in [
            InstallMethod::Dev,
            InstallMethod::CargoPath {
                path: "/src".to_owned(),
            },
            InstallMethod::Unknown {
                path: PathBuf::from("/x/deckhand"),
            },
        ] {
            assert_eq!(upgrade_command(&left, "3.4.0"), None, "{left:?}");
            assert!(!manual_instructions(&left, "3.4.0").is_empty(), "{left:?}");
        }
    }
}
