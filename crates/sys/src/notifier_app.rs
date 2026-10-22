//! Not in the TS: the app bundle that gives Deckhand's macOS notifications its own icon.
//!
//! Notification Center shows the sending app's icon, and `osascript` sends as Script Editor. It only accepts an app
//! LaunchServices knows that lives outside the temporary directories, so the daemon keeps `<config dir>/Deckhand.app`:
//! an `LSUIElement` bundle with the brand icon whose executable is a copy of this binary, signed ad hoc and registered
//! with `lsregister`. The daemon runs that copy with [`POST_FLAG`] to post (the deckhand binary's `notifier` module).
//! The bundle is rebuilt when the binary it copied changes, so an upgrade carries over on the next daemon start.

use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

use tokio::sync::OnceCell;

use crate::paths;
use crate::proc::{self, ExecOptions};

/// The bundle identifier Notification Center and System Settings › Notifications know Deckhand by.
pub const BUNDLE_ID: &str = "io.github.tejgor.deckhand";

/// The bundle's directory name in the config dir.
pub const APP_DIR: &str = "Deckhand.app";

/// The bundle's executable (`Contents/MacOS/deckhand`).
pub const EXECUTABLE: &str = "deckhand";

/// Run as `<bundle executable> --notify <title> <body>`, the copy posts one notification and exits.
pub const POST_FLAG: &str = "--notify";

/// Run as `<bundle executable> --notify-status`, the copy prints macOS's answer for Deckhand: [`STATUS_ALLOWED`],
/// [`STATUS_DENIED`] or [`STATUS_NOT_ASKED`]. A bundle built from an older binary prints nothing (it ignores the flag),
/// which reads as "unknown", never as allowed.
pub const STATUS_FLAG: &str = "--notify-status";
pub const STATUS_ALLOWED: &str = "allowed";
pub const STATUS_DENIED: &str = "denied";
pub const STATUS_NOT_ASKED: &str = "not-asked";

/// The exit status of a post the user has turned off for Deckhand (System Settings › Notifications): nothing was shown
/// and nothing should be shown in its place.
pub const EXIT_DENIED: u8 = 3;

/// The exit status while the permission prompt is unanswered: nothing was shown, and nothing should be shown in its
/// place (osascript would ask again, as Script Editor).
pub const EXIT_NOT_ASKED: u8 = 4;

/// The brand mark as an icon set (built by `scripts/make-icns.swift` from `assets/deckhand.png`).
const ICON: &[u8] = include_bytes!("../../../assets/Deckhand.icns");

/// What the bundle was built from (see [`build_stamp`]), inside the signed bundle.
const STAMP: &str = "Contents/Resources/deckhand-build";

const LSREGISTER: &str =
    "/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister";

/// For `codesign` and `lsregister`, which take well under a second.
const TOOL_TIMEOUT: Duration = Duration::from_secs(20);

/// Where Notification Center refuses to look (LaunchServices flags these bundles `in-temp-dir`).
const TEMPORARY_ROOTS: &[&str] = &["/tmp", "/private/tmp", "/var/folders", "/private/var/folders"];

static READY: OnceCell<Result<PathBuf, String>> = OnceCell::const_new();

/// The bundled executable to post with. The first call in a process builds or refreshes the bundle; the answer
/// (including why there is none) is kept for the life of the process.
pub async fn notifier_executable() -> Result<PathBuf, String> {
    READY
        .get_or_init(|| async {
            let app = paths::config_dir().join(APP_DIR);
            let exe = std::env::current_exe().map_err(|error| error.to_string())?;
            ensure_app(&exe, &app).await.map_err(|error| error.to_string())?;
            Ok(executable_path(&app))
        })
        .await
        .clone()
}

/// The bundled executable when the bundle is already there, without building it (for read-only checks).
pub fn existing_executable() -> Option<PathBuf> {
    let executable = executable_path(&paths::config_dir().join(APP_DIR));
    executable.is_file().then_some(executable)
}

/// `<app>/Contents/MacOS/deckhand`.
pub fn executable_path(app: &Path) -> PathBuf {
    app.join("Contents/MacOS").join(EXECUTABLE)
}

/// Builds `app` from `exe` unless it already holds that build: assembled beside it, signed, swapped in, registered.
pub async fn ensure_app(exe: &Path, app: &Path) -> io::Result<()> {
    if is_temporary(app) {
        return Err(io::Error::other(format!(
            "{} is in a temporary directory, where Notification Center ignores apps",
            app.display()
        )));
    }
    let stamp = build_stamp(exe)?;
    if fs::read_to_string(app.join(STAMP)).is_ok_and(|built| built == stamp) {
        return Ok(());
    }
    let parent = app.parent().unwrap_or(Path::new("/"));
    let staging = parent.join(format!(".{APP_DIR}.{}", std::process::id()));
    let result = install(exe, app, &staging, stamp).await;
    if result.is_err() {
        let _ = tokio::fs::remove_dir_all(&staging).await;
    }
    result
}

async fn install(exe: &Path, app: &Path, staging: &Path, stamp: String) -> io::Result<()> {
    let (from, to) = (exe.to_path_buf(), staging.to_path_buf());
    tokio::task::spawn_blocking(move || write_bundle(&from, &to, &stamp))
        .await
        .map_err(io::Error::other)??;
    let options = ExecOptions::default().timeout(TOOL_TIMEOUT);
    let staging_arg = staging.as_os_str();
    proc::exec(
        "/usr/bin/codesign",
        &["--force".as_ref(), "--sign".as_ref(), "-".as_ref(), staging_arg],
        &options,
    )
    .await
    .map_err(io::Error::other)?;
    match tokio::fs::remove_dir_all(app).await {
        Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
        _ => {}
    }
    tokio::fs::rename(staging, app).await?;
    proc::exec(LSREGISTER, &["-f".as_ref(), app.as_os_str()], &options)
        .await
        .map_err(io::Error::other)?;
    Ok(())
}

/// Lays out an unsigned bundle at `dir` (replacing whatever is there): Info.plist, the icon, the executable copied
/// from `exe`, and the stamp.
pub fn write_bundle(exe: &Path, dir: &Path, stamp: &str) -> io::Result<()> {
    match fs::remove_dir_all(dir) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
        _ => {}
    }
    let resources = dir.join("Contents/Resources");
    fs::create_dir_all(&resources)?;
    fs::create_dir_all(dir.join("Contents/MacOS"))?;
    fs::write(dir.join("Contents/Info.plist"), info_plist(env!("CARGO_PKG_VERSION")))?;
    fs::write(resources.join("Deckhand.icns"), ICON)?;
    let executable = executable_path(dir);
    fs::copy(exe, &executable)?;
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o755))?;
    fs::write(dir.join(STAMP), stamp)
}

/// The bundle's Info.plist: a background (`LSUIElement`) app named Deckhand with the brand icon.
pub fn info_plist(version: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleIdentifier</key>
	<string>{BUNDLE_ID}</string>
	<key>CFBundleName</key>
	<string>Deckhand</string>
	<key>CFBundleDisplayName</key>
	<string>Deckhand</string>
	<key>CFBundleExecutable</key>
	<string>{EXECUTABLE}</string>
	<key>CFBundleIconFile</key>
	<string>Deckhand</string>
	<key>CFBundlePackageType</key>
	<string>APPL</string>
	<key>CFBundleShortVersionString</key>
	<string>{version}</string>
	<key>CFBundleVersion</key>
	<string>{version}</string>
	<key>LSUIElement</key>
	<true/>
</dict>
</plist>
"#
    )
}

/// Identifies the binary a bundle was built from: this version, and the size and modification time of `exe`.
pub fn build_stamp(exe: &Path) -> io::Result<String> {
    let metadata = fs::metadata(exe)?;
    let modified = metadata
        .modified()?
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    Ok(format!("{} {} {modified}\n", env!("CARGO_PKG_VERSION"), metadata.len()))
}

/// Whether `path` (or, while it does not exist, its parent) resolves into a temporary directory.
pub fn is_temporary(path: &Path) -> bool {
    let resolved = fs::canonicalize(path)
        .or_else(|_| path.parent().map_or(Ok(path.to_path_buf()), fs::canonicalize))
        .unwrap_or_else(|_| path.to_path_buf());
    let temp_dir = fs::canonicalize(std::env::temp_dir()).ok();
    TEMPORARY_ROOTS
        .iter()
        .map(Path::new)
        .chain(temp_dir.as_deref())
        .any(|root| resolved.starts_with(root))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn info_plist_names_a_background_app_with_the_brand_icon() {
        let plist = info_plist("3.1.0");
        assert!(plist.contains("<string>io.github.tejgor.deckhand</string>"));
        assert!(plist.contains("<key>CFBundleExecutable</key>\n\t<string>deckhand</string>"));
        assert!(plist.contains("<key>CFBundleIconFile</key>\n\t<string>Deckhand</string>"));
        assert!(plist.contains("<key>CFBundleVersion</key>\n\t<string>3.1.0</string>"));
        assert!(plist.contains("<key>LSUIElement</key>\n\t<true/>"));
    }

    #[test]
    fn write_bundle_lays_out_an_app_around_a_copy_of_the_binary() {
        let root = tempfile::tempdir().unwrap();
        let exe = root.path().join("deckhand-bin");
        fs::write(&exe, b"binary").unwrap();
        let app = root.path().join(APP_DIR);
        fs::create_dir_all(app.join("Contents/stale")).unwrap();
        let stamp = build_stamp(&exe).unwrap();
        write_bundle(&exe, &app, &stamp).unwrap();
        assert!(!app.join("Contents/stale").exists());
        let executable = executable_path(&app);
        assert_eq!(fs::read(&executable).unwrap(), b"binary");
        assert_eq!(fs::metadata(&executable).unwrap().permissions().mode() & 0o777, 0o755);
        assert_eq!(fs::read(app.join("Contents/Resources/Deckhand.icns")).unwrap(), ICON);
        assert!(
            fs::read_to_string(app.join("Contents/Info.plist"))
                .unwrap()
                .contains(BUNDLE_ID)
        );
        assert_eq!(fs::read_to_string(app.join(STAMP)).unwrap(), stamp);
    }

    #[test]
    fn the_stamp_changes_with_the_binary() {
        let root = tempfile::tempdir().unwrap();
        let exe = root.path().join("deckhand-bin");
        fs::write(&exe, b"one").unwrap();
        let before = build_stamp(&exe).unwrap();
        assert!(before.starts_with(concat!(env!("CARGO_PKG_VERSION"), " 3 ")));
        fs::write(&exe, b"three").unwrap();
        assert_ne!(build_stamp(&exe).unwrap(), before);
    }

    #[test]
    fn temporary_directories_are_recognised() {
        let root = tempfile::tempdir().unwrap();
        assert!(is_temporary(&root.path().join(APP_DIR)));
        assert!(is_temporary(Path::new("/private/tmp/x/Deckhand.app")));
        assert!(!is_temporary(Path::new("/Users/someone/.deckhand/Deckhand.app")));
    }

    #[tokio::test]
    async fn no_bundle_is_built_in_a_temporary_directory() {
        let root = tempfile::tempdir().unwrap();
        let exe = root.path().join("deckhand-bin");
        fs::write(&exe, b"binary").unwrap();
        let app = root.path().join(APP_DIR);
        let error = ensure_app(&exe, &app).await.unwrap_err();
        assert!(error.to_string().contains("temporary directory"));
        assert!(!app.exists());
    }
}
