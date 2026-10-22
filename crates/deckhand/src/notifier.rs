//! Not in the TS: posting a notification as Deckhand.app (see `deckhand_sys::notifier_app`).
//!
//! The daemon runs the bundle's copy of this binary as `deckhand --notify <title> <body>`. Notification Center
//! attributes what that process posts to the bundle, so the notification carries Deckhand's icon. Run as
//! `deckhand --notify-status` it reports macOS's answer for the bundle without asking. Started from the bundle any
//! other way (clicking one of its notifications launches the app) it has nothing to do and exits.

use std::path::Path;
use std::process::ExitCode;
use std::ptr::NonNull;
use std::sync::mpsc;
use std::time::Duration;

use block2::RcBlock;
use deckhand_sys::notifier_app::{
    APP_DIR, EXECUTABLE, EXIT_DENIED, EXIT_NOT_ASKED, POST_FLAG, STATUS_ALLOWED, STATUS_DENIED, STATUS_FLAG,
    STATUS_NOT_ASKED,
};
use objc2::runtime::Bool;
use objc2_foundation::{NSError, NSString};
use objc2_user_notifications::{
    UNAuthorizationOptions, UNAuthorizationStatus, UNMutableNotificationContent, UNNotificationRequest,
    UNNotificationSettings, UNUserNotificationCenter,
};

/// The first post may wait on the user answering the permission prompt.
const AUTHORIZATION_TIMEOUT: Duration = Duration::from_secs(20);

const POST_TIMEOUT: Duration = Duration::from_secs(5);

/// Reading the current answer never prompts.
const SETTINGS_TIMEOUT: Duration = Duration::from_secs(5);

/// macOS's answer for Deckhand.app.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Answer {
    Allowed,
    /// Turned off in System Settings › Notifications (or "Don't Allow" in the prompt).
    Denied,
    /// The prompt was never answered (or never shown).
    NotAsked,
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
        [flag, ..] if flag == STATUS_FLAG => status(),
        _ => ExitCode::SUCCESS,
    })
}

/// Whether `exe` is `…/Deckhand.app/Contents/MacOS/deckhand`.
fn is_bundled(exe: &Path) -> bool {
    exe.ends_with(Path::new(APP_DIR).join("Contents/MacOS").join(EXECUTABLE))
}

fn exit_code(answer: Answer) -> ExitCode {
    match answer {
        Answer::Allowed => ExitCode::SUCCESS,
        Answer::Denied => ExitCode::from(EXIT_DENIED),
        Answer::NotAsked => ExitCode::from(EXIT_NOT_ASKED),
    }
}

fn answer_of(status: UNAuthorizationStatus) -> Answer {
    match status {
        UNAuthorizationStatus::Denied => Answer::Denied,
        UNAuthorizationStatus::NotDetermined => Answer::NotAsked,
        // Authorized, and the quiet kinds (provisional, ephemeral) that still deliver.
        _ => Answer::Allowed,
    }
}

/// macOS's current answer, without prompting. `None` when it doesn't say in time.
fn current_answer(center: &UNUserNotificationCenter) -> Option<Answer> {
    let (sender, receiver) = mpsc::channel();
    let handler = RcBlock::new(move |settings: NonNull<UNNotificationSettings>| {
        // SAFETY: the framework passes a valid settings object for the duration of the call.
        let status = unsafe { settings.as_ref() }.authorizationStatus();
        let _ = sender.send(answer_of(status));
    });
    center.getNotificationSettingsWithCompletionHandler(&handler);
    receiver.recv_timeout(SETTINGS_TIMEOUT).ok()
}

/// `--notify-status`: prints macOS's answer ([`STATUS_ALLOWED`], [`STATUS_DENIED`], [`STATUS_NOT_ASKED`]); 1 when
/// macOS doesn't answer.
fn status() -> ExitCode {
    let center = UNUserNotificationCenter::currentNotificationCenter();
    let Some(answer) = current_answer(&center) else {
        return ExitCode::FAILURE;
    };
    println!("{}", status_word(answer));
    ExitCode::SUCCESS
}

fn status_word(answer: Answer) -> &'static str {
    match answer {
        Answer::Allowed => STATUS_ALLOWED,
        Answer::Denied => STATUS_DENIED,
        Answer::NotAsked => STATUS_NOT_ASKED,
    }
}

/// Asks for permission (macOS shows its prompt the first time): the answer, or `None` on an error.
fn request_permission(center: &UNUserNotificationCenter) -> Option<Answer> {
    let (sender, receiver) = mpsc::channel();
    let handler = RcBlock::new(move |granted: Bool, error: *mut NSError| {
        let _ = sender.send(match (granted.as_bool(), error.is_null()) {
            (true, _) => Some(Answer::Allowed),
            (false, true) => Some(Answer::Denied),
            (false, false) => None,
        });
    });
    center.requestAuthorizationWithOptions_completionHandler(UNAuthorizationOptions::Alert, &handler);
    match receiver.recv_timeout(AUTHORIZATION_TIMEOUT) {
        Ok(answer) => answer,
        // The prompt is still up: nobody answered it in time.
        Err(_) => Some(Answer::NotAsked),
    }
}

/// Posts `title` / `body` now: 0 once Notification Center took it, [`EXIT_DENIED`] when the user turned Deckhand's
/// notifications off, [`EXIT_NOT_ASKED`] while the permission prompt is unanswered, 1 otherwise (the daemon then
/// falls back to osascript).
fn post(title: &str, body: &str) -> ExitCode {
    let center = UNUserNotificationCenter::currentNotificationCenter();
    match current_answer(&center) {
        Some(Answer::Allowed) => {}
        Some(Answer::Denied) => return exit_code(Answer::Denied),
        // Not answered yet (or macOS didn't say): ask, which shows the prompt the first time.
        Some(Answer::NotAsked) | None => match request_permission(&center) {
            Some(Answer::Allowed) => {}
            // Refused without an error: either "Don't Allow", or a prompt left unanswered.
            Some(Answer::Denied) => {
                return exit_code(current_answer(&center).unwrap_or(Answer::Denied));
            }
            Some(Answer::NotAsked) => return exit_code(Answer::NotAsked),
            None => return ExitCode::FAILURE,
        },
    }
    let (sender, receiver) = mpsc::channel();
    let content = UNMutableNotificationContent::new();
    content.setTitle(&NSString::from_str(title));
    content.setBody(&NSString::from_str(body));
    let identifier = NSString::from_str(&uuid::Uuid::new_v4().to_string());
    let request = UNNotificationRequest::requestWithIdentifier_content_trigger(&identifier, &content, None);
    let added = RcBlock::new(move |error: *mut NSError| {
        let _ = sender.send(error.is_null());
    });
    center.addNotificationRequest_withCompletionHandler(&request, Some(&added));
    match receiver.recv_timeout(POST_TIMEOUT) {
        Ok(true) => ExitCode::SUCCESS,
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
        assert!(run_if_bundled(&[STATUS_FLAG.to_owned()]).is_none());
    }

    #[test]
    fn every_status_maps_to_an_answer() {
        assert_eq!(answer_of(UNAuthorizationStatus::Authorized), Answer::Allowed);
        assert_eq!(answer_of(UNAuthorizationStatus::Provisional), Answer::Allowed);
        assert_eq!(answer_of(UNAuthorizationStatus::Ephemeral), Answer::Allowed);
        assert_eq!(answer_of(UNAuthorizationStatus::Denied), Answer::Denied);
        assert_eq!(answer_of(UNAuthorizationStatus::NotDetermined), Answer::NotAsked);
        assert_eq!(
            [Answer::Allowed, Answer::Denied, Answer::NotAsked].map(status_word),
            [STATUS_ALLOWED, STATUS_DENIED, STATUS_NOT_ASKED]
        );
    }
}
