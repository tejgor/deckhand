//! Not in the TS as a module: desktop notifications, shared by the daemon (a session needs you or exits) and the UI
//! (the notification that confirms they are on, which also brings up macOS's permission prompt while you look).
//!
//! On macOS they are posted by Deckhand.app (see [`crate::notifier_app`]) so they carry Deckhand's icon, and by
//! osascript only when that app cannot post. Elsewhere they use notify-send. Best effort throughout.

use std::time::Duration;

use deckhand_protocol::data::NotificationPermission;

use crate::proc::{self, ExecOptions};

/// What became of a notification.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Delivery {
    /// Handed to the system.
    Sent,
    /// macOS: turned off for Deckhand in System Settings › Notifications; nothing is shown in its place.
    Blocked,
    /// macOS: the permission prompt is unanswered; nothing is shown in its place.
    NotAsked,
    /// macOS: Deckhand.app could not post, so osascript was tried instead. Why, for the daemon log.
    Fallback(String),
}

/// The osascript program behind a notification (title and text passed as arguments, never interpolated).
#[cfg(target_os = "macos")]
const OSASCRIPT_NOTIFY: &str =
    "on run argv\ndisplay notification (item 2 of argv) with title (item 1 of argv)\nend run";

/// TS: notify's execFile `timeout`.
const NOTIFY_TIMEOUT: Duration = Duration::from_millis(2000);

/// Posting through Deckhand.app can wait on the user answering the first permission prompt.
#[cfg(target_os = "macos")]
const NOTIFIER_TIMEOUT: Duration = Duration::from_secs(30);

/// Reading the permission never prompts, so it answers at once.
#[cfg(target_os = "macos")]
const STATUS_TIMEOUT: Duration = Duration::from_secs(10);

/// Shows `title` / `body` as a desktop notification.
pub async fn send(title: &str, body: &str) -> Delivery {
    let options = ExecOptions::default().timeout(NOTIFY_TIMEOUT);
    #[cfg(target_os = "macos")]
    {
        use crate::notifier_app::{EXIT_DENIED, EXIT_NOT_ASKED, POST_FLAG, notifier_executable};
        let reason = match notifier_executable().await {
            Ok(executable) => {
                let app_options = ExecOptions::default().timeout(NOTIFIER_TIMEOUT);
                match proc::exec(&executable.to_string_lossy(), &[POST_FLAG, title, body], &app_options).await {
                    Ok(_) => return Delivery::Sent,
                    Err(error) if error.exit_code() == Some(i32::from(EXIT_DENIED)) => return Delivery::Blocked,
                    Err(error) if error.exit_code() == Some(i32::from(EXIT_NOT_ASKED)) => return Delivery::NotAsked,
                    Err(error) => format!("Deckhand.app could not post a notification: {error}"),
                }
            }
            Err(error) => format!("notifications fall back to osascript: {error}"),
        };
        let _ = proc::exec("osascript", &["-e", OSASCRIPT_NOTIFY, title, body], &options).await;
        Delivery::Fallback(reason)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = proc::exec("notify-send", &["--", title, body], &options).await;
        Delivery::Sent
    }
}

/// macOS's answer for Deckhand.app. With `build`, the app is built first when needed (the daemon, which posts with
/// it anyway); without, only an app that is already there is asked, so a read-only check writes nothing. `None`: not
/// macOS, no app, or no answer.
pub async fn permission(build: bool) -> Option<NotificationPermission> {
    #[cfg(target_os = "macos")]
    {
        use crate::notifier_app::{
            STATUS_ALLOWED, STATUS_DENIED, STATUS_FLAG, STATUS_NOT_ASKED, existing_executable, notifier_executable,
        };
        let executable = if build {
            notifier_executable().await.ok()?
        } else {
            existing_executable()?
        };
        let options = ExecOptions::default().timeout(STATUS_TIMEOUT);
        let output = proc::exec(&executable.to_string_lossy(), &[STATUS_FLAG], &options)
            .await
            .ok()?;
        // Only an explicit answer counts: an app built from an older binary ignores the flag and prints nothing.
        match output.stdout.trim() {
            STATUS_ALLOWED => Some(NotificationPermission::Allowed),
            STATUS_DENIED => Some(NotificationPermission::Blocked),
            STATUS_NOT_ASKED => Some(NotificationPermission::NotAsked),
            _ => None,
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = build;
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_read_only_check_without_an_app_has_no_answer() {
        use crate::storage::tests::{Home, block_on};
        let dir = tempfile::tempdir().unwrap();
        let _home = Home::set(dir.path());
        assert_eq!(block_on(permission(false)), None);
    }
}
