//! Not in the TS: posting a notification as Deckhand.app (see `deckhand_sys::notifier_app`).
//!
//! The daemon runs the bundle's copy of this binary as `deckhand --notify <title> <body>`. Notification Center
//! attributes what that process posts to the bundle, so the notification carries Deckhand's icon. Started from the
//! bundle any other way (clicking one of its notifications launches the app) it has nothing to do and exits.

use std::path::Path;
use std::process::ExitCode;
use std::sync::mpsc;
use std::time::Duration;

use block2::RcBlock;
use deckhand_sys::notifier_app::{APP_DIR, EXECUTABLE, EXIT_DENIED, POST_FLAG};
use objc2::runtime::Bool;
use objc2_foundation::{NSError, NSString};
use objc2_user_notifications::{
    UNAuthorizationOptions, UNMutableNotificationContent, UNNotificationRequest, UNUserNotificationCenter,
};

/// The first post may wait on the user answering the permission prompt.
const AUTHORIZATION_TIMEOUT: Duration = Duration::from_secs(20);

const POST_TIMEOUT: Duration = Duration::from_secs(5);

/// How a completion handler ended.
enum Outcome {
    Done,
    /// Authorization refused without an error: the user turned Deckhand's notifications off.
    Denied,
    Failed,
}

/// When this process is the bundle's executable: its exit status (after posting, for [`POST_FLAG`]). `None` for the
/// ordinary binary.
pub fn run_if_bundled(args: &[String]) -> Option<ExitCode> {
    let exe = std::env::current_exe().ok()?;
    if !is_bundled(&exe) {
        return None;
    }
    Some(match args {
        [flag, title, body, ..] if flag == POST_FLAG => post(title, body),
        _ => ExitCode::SUCCESS,
    })
}

/// Whether `exe` is `…/Deckhand.app/Contents/MacOS/deckhand`.
fn is_bundled(exe: &Path) -> bool {
    exe.ends_with(Path::new(APP_DIR).join("Contents/MacOS").join(EXECUTABLE))
}

/// Posts `title` / `body` now: 0 once Notification Center took it, [`EXIT_DENIED`] when the user turned Deckhand's
/// notifications off, 1 otherwise (the daemon then falls back to osascript).
fn post(title: &str, body: &str) -> ExitCode {
    let center = UNUserNotificationCenter::currentNotificationCenter();
    let (sender, receiver) = mpsc::channel();
    let authorized = {
        let sender = sender.clone();
        RcBlock::new(move |granted: Bool, error: *mut NSError| {
            let outcome = match (granted.as_bool(), error.is_null()) {
                (true, _) => Outcome::Done,
                (false, true) => Outcome::Denied,
                (false, false) => Outcome::Failed,
            };
            let _ = sender.send(outcome);
        })
    };
    center.requestAuthorizationWithOptions_completionHandler(UNAuthorizationOptions::Alert, &authorized);
    match receiver.recv_timeout(AUTHORIZATION_TIMEOUT) {
        Ok(Outcome::Done) => {}
        Ok(Outcome::Denied) => return ExitCode::from(EXIT_DENIED),
        Ok(Outcome::Failed) | Err(_) => return ExitCode::FAILURE,
    }
    let content = UNMutableNotificationContent::new();
    content.setTitle(&NSString::from_str(title));
    content.setBody(&NSString::from_str(body));
    let identifier = NSString::from_str(&uuid::Uuid::new_v4().to_string());
    let request = UNNotificationRequest::requestWithIdentifier_content_trigger(&identifier, &content, None);
    let added = RcBlock::new(move |error: *mut NSError| {
        let _ = sender.send(if error.is_null() {
            Outcome::Done
        } else {
            Outcome::Failed
        });
    });
    center.addNotificationRequest_withCompletionHandler(&request, Some(&added));
    match receiver.recv_timeout(POST_TIMEOUT) {
        Ok(Outcome::Done) => ExitCode::SUCCESS,
        _ => ExitCode::FAILURE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_bundle_executable_counts_as_bundled() {
        assert!(is_bundled(Path::new(
            "/Users/u/.deckhand/Deckhand.app/Contents/MacOS/deckhand"
        )));
        assert!(!is_bundled(Path::new("/Users/u/.cargo/bin/deckhand")));
        assert!(!is_bundled(Path::new("/Users/u/Other.app/Contents/MacOS/deckhand")));
    }

    #[test]
    fn the_ordinary_binary_is_not_bundled() {
        assert!(run_if_bundled(&[POST_FLAG.to_owned(), "t".to_owned(), "b".to_owned()]).is_none());
    }
}
