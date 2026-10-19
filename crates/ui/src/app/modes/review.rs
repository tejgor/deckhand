//! Port of app.tsx's review-project keys: the inline repository-config review. Enter trusts these exact bytes and
//! resumes the gated action; s resumes it with the override ignored (or cancels); Esc closes (back where it came
//! from). A config that changed since the review shows the new bytes instead.

use deckhand_core::display::error_message;
use deckhand_core::key::Input;

use crate::app::state::{App, AppCallback, Mode, ReviewSkip};
use crate::keymap::{Binding, HelpRef, KeySpec, Keymap, NamedKey};

/// review-project keys.
pub static KEYMAP: Keymap = &[
    Binding {
        keys: &[KeySpec::Named(NamedKey::Return)],
        action: "trust these exact bytes and resume the gated action",
        help: HelpRef::Hidden("the review's own footer names it (enter trust & continue)"),
    },
    Binding {
        keys: &[KeySpec::Char("s")],
        action: "resume with the override ignored, or cancel",
        help: HelpRef::Hidden("the review's own footer names it (s continue without it)"),
    },
    Binding {
        keys: &[KeySpec::Named(NamedKey::Escape)],
        action: "close the review, back where it came from",
        help: HelpRef::Hidden("the review's own footer names it (esc cancel / back / close)"),
    },
];

/// TS `close` with the `back` captured when the key was pressed: forget the review, back to browse, then `back`.
fn close_with(app: &mut App, back: Option<AppCallback>) {
    app.review.review = None;
    app.set_mode(Mode::Browse);
    if let Some(back) = back {
        back(app);
    }
}

/// TS: the review's `close`: forget it, back to browse, then its `back`.
pub fn close_review(app: &mut App) {
    let back = app.review.review.as_ref().and_then(|review| review.back.clone());
    close_with(app, back);
}

/// TS: the review-project branch of the useTerminalInput handler.
pub fn handle_review_project(app: &mut App, input: &Input) {
    let key = &input.key;
    // Enter trusts these exact bytes and resumes the gated action; s resumes it with the override ignored.
    let Some(review) = app.review.review.clone() else {
        app.set_mode(Mode::Browse);
        return;
    };
    let project = review.project.clone();
    if key.escape {
        close_with(app, review.back.clone());
        return;
    }
    if app.scroll_details_pane(input) {
        return;
    }
    if input.input == "s" {
        close_with(app, review.back.clone());
        match &review.skip {
            ReviewSkip::Cancel => {}
            ReviewSkip::Resume => {
                if let Some(resume) = &review.resume {
                    resume(app, Some(project));
                }
            }
            ReviewSkip::Run(skip) => skip(app, project),
        }
        return;
    }
    let (true, Some(client)) = (key.return_, app.client()) else {
        return;
    };
    if !project.needs_review {
        close_with(app, review.back.clone());
        app.set_status(Some(
            if project.trusted {
                "Already trusted"
            } else {
                "Nothing to trust"
            }
            .to_owned(),
        ));
        if let Some(resume) = &review.resume {
            resume(app, Some(project));
        }
        return;
    }
    app.set_busy(true);
    app.set_error(None);
    let reply = client.trust_project(&review.cwd, &project.project.fingerprint);
    let cwd = review.cwd.clone();
    let task = app.dispatch().spawn(reply, move |app: &mut App, result| match result {
        Ok(trusted) => {
            app.set_busy(false);
            close_with(app, review.back.clone());
            app.set_status(Some("Repository configuration trusted".to_owned()));
            if let Some(resume) = &review.resume {
                resume(app, Some(trusted));
            }
        }
        Err(error) => {
            // Changed since this review: show the new bytes instead.
            app.set_error(Some(error_message(&error)));
            let reply = client.project_info(&cwd);
            let refresh = app.dispatch().spawn(reply, |app: &mut App, result| {
                if let (Ok(next), Some(current)) = (result, app.review.review.as_mut()) {
                    current.project = next;
                }
                app.set_busy(false);
            });
            drop(refresh);
        }
    });
    drop(task);
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use serde_json::json;

    use super::*;
    use crate::app::modes::workspace_info::fake_daemon::{connect, local, project_info, settle};
    use crate::app::state::{AppAction, AppProps, Review};
    use crate::app::test_support::app;
    use deckhand_core::key::Key;
    use deckhand_protocol::data::ProjectInfo;

    type Log = Rc<RefCell<Vec<String>>>;

    fn project(fingerprint: &str, trusted: bool, needs_review: bool) -> ProjectInfo {
        let mut value = project_info(json!({"devCommand": "echo dev"}), json!({}), trusted, needs_review);
        value["fingerprint"] = json!(fingerprint);
        serde_json::from_value(value).unwrap_or_else(|error| panic!("{error}"))
    }

    fn enter() -> Input {
        Input::key(
            "",
            Key {
                return_: true,
                ..Key::NONE
            },
        )
    }

    fn escape() -> Input {
        Input::key(
            "",
            Key {
                escape: true,
                ..Key::NONE
            },
        )
    }

    /// An action that records `<name>:<fingerprint of the project, or none>`.
    fn recorder(log: &Log, name: &'static str) -> AppAction<Option<ProjectInfo>> {
        let log = Rc::clone(log);
        Rc::new(move |_: &mut App, project: Option<ProjectInfo>| {
            let fingerprint = project.map_or_else(|| "none".to_owned(), |project| project.project.fingerprint);
            log.borrow_mut().push(format!("{name}:{fingerprint}"));
        })
    }

    fn review(project: ProjectInfo, resume: Option<AppAction<Option<ProjectInfo>>>, skip: ReviewSkip) -> Review {
        Review {
            project,
            cwd: "/repo".to_owned(),
            resume,
            skip,
            labels: None,
            purpose: None,
            back: None,
        }
    }

    fn open(app: &mut App, review: Review) {
        app.review.review = Some(review);
        app.set_mode(Mode::ReviewProject);
    }

    #[test]
    fn without_a_review_the_mode_leaves_for_browse() {
        let (mut app, _rx) = app(AppProps::default());
        app.set_mode(Mode::ReviewProject);
        handle_review_project(&mut app, &Input::text("s"));
        assert_eq!(app.mode, Mode::Browse);
    }

    #[test]
    fn escape_closes_and_runs_back() {
        let (mut app, _rx) = app(AppProps::default());
        let log: Log = Rc::default();
        let back_log = Rc::clone(&log);
        let mut shown = review(
            project("fp", false, true),
            Some(recorder(&log, "resume")),
            ReviewSkip::Resume,
        );
        shown.back = Some(Rc::new(move |app: &mut App| {
            back_log.borrow_mut().push("back".to_owned());
            app.set_mode(Mode::PickAction);
        }));
        open(&mut app, shown);
        handle_review_project(&mut app, &escape());
        assert!(app.review.review.is_none());
        // `back` runs after browse is set, so it can return somewhere else.
        assert_eq!(app.mode, Mode::PickAction);
        assert_eq!(*log.borrow(), ["back"]);
    }

    #[test]
    fn s_resumes_without_the_override_or_cancels_or_runs_its_own_skip() {
        let (mut app, _rx) = app(AppProps::default());
        let log: Log = Rc::default();
        open(
            &mut app,
            review(
                project("a", false, true),
                Some(recorder(&log, "resume")),
                ReviewSkip::Resume,
            ),
        );
        handle_review_project(&mut app, &Input::text("s"));
        assert_eq!((app.mode, app.review.review.is_none()), (Mode::Browse, true));
        open(
            &mut app,
            review(
                project("b", false, true),
                Some(recorder(&log, "resume")),
                ReviewSkip::Cancel,
            ),
        );
        handle_review_project(&mut app, &Input::text("s"));
        assert_eq!(app.mode, Mode::Browse);
        let skip_log = Rc::clone(&log);
        let skip: AppAction<ProjectInfo> = Rc::new(move |_: &mut App, project: ProjectInfo| {
            skip_log
                .borrow_mut()
                .push(format!("skip:{}", project.project.fingerprint));
        });
        open(
            &mut app,
            review(
                project("c", false, true),
                Some(recorder(&log, "resume")),
                ReviewSkip::Run(skip),
            ),
        );
        handle_review_project(&mut app, &Input::text("s"));
        // No gated action (an explicit T review): s only closes.
        open(&mut app, review(project("d", false, true), None, ReviewSkip::Resume));
        handle_review_project(&mut app, &Input::text("s"));
        assert_eq!(*log.borrow(), ["resume:a", "skip:c"]);
    }

    #[test]
    fn enter_on_a_config_with_nothing_to_trust_just_resumes() {
        local(async {
            let (mut app, _rx) = app(AppProps::default());
            let _seen = connect(&mut app, |_| Err("never asked".to_owned()));
            let log: Log = Rc::default();
            open(
                &mut app,
                review(
                    project("t", true, false),
                    Some(recorder(&log, "resume")),
                    ReviewSkip::Resume,
                ),
            );
            handle_review_project(&mut app, &enter());
            assert_eq!(app.messages.status_message.as_deref(), Some("Already trusted"));
            assert_eq!(
                (app.mode, app.review.review.is_none(), app.messages.busy),
                (Mode::Browse, true, false)
            );
            open(
                &mut app,
                review(
                    project("n", false, false),
                    Some(recorder(&log, "resume")),
                    ReviewSkip::Resume,
                ),
            );
            handle_review_project(&mut app, &enter());
            assert_eq!(app.messages.status_message.as_deref(), Some("Nothing to trust"));
            assert_eq!(*log.borrow(), ["resume:t", "resume:n"]);
        });
    }

    #[test]
    fn enter_needs_a_connection() {
        let (mut app, _rx) = app(AppProps::default());
        let log: Log = Rc::default();
        open(
            &mut app,
            review(
                project("t", false, true),
                Some(recorder(&log, "resume")),
                ReviewSkip::Resume,
            ),
        );
        handle_review_project(&mut app, &enter());
        assert_eq!(app.mode, Mode::ReviewProject);
        assert!(log.borrow().is_empty() && !app.messages.busy);
    }

    #[test]
    fn enter_trusts_these_bytes_then_resumes_with_the_trusted_project() {
        local(async {
            let (mut app, mut rx) = app(AppProps::default());
            let seen = connect(&mut app, |message| {
                assert_eq!(message["type"], "trust-project");
                Ok(project_info(
                    json!({"devCommand": "echo dev"}),
                    json!({"devCommand": "echo dev"}),
                    true,
                    false,
                ))
            });
            let log: Log = Rc::default();
            let mut shown = review(
                project("fp-reviewed", false, true),
                Some(recorder(&log, "resume")),
                ReviewSkip::Resume,
            );
            shown.cwd = "/repo/sub".to_owned();
            open(&mut app, shown);
            app.set_error(Some("old".to_owned()));
            handle_review_project(&mut app, &enter());
            assert!(app.messages.busy);
            assert_eq!(app.messages.error, None);
            // Busy ignores keys.
            app.handle_input(&escape());
            assert_eq!(app.mode, Mode::ReviewProject);
            settle(&mut app, &mut rx).await;
            assert!(!app.messages.busy && app.review.review.is_none());
            assert_eq!(app.mode, Mode::Browse);
            assert_eq!(
                app.messages.status_message.as_deref(),
                Some("Repository configuration trusted")
            );
            // The fixture's answer carries the same fingerprint as the review.
            assert_eq!(*log.borrow(), ["resume:fp1"]);
            let seen = seen.borrow();
            assert_eq!(seen[0]["cwd"], "/repo/sub");
            assert_eq!(seen[0]["fingerprint"], "fp-reviewed");
        });
    }

    #[test]
    fn a_config_changed_since_the_review_shows_the_new_bytes() {
        local(async {
            let (mut app, mut rx) = app(AppProps::default());
            let seen = connect(&mut app, |message| match message["type"].as_str() {
                Some("trust-project") => Err("repository configuration changed".to_owned()),
                _ => {
                    let mut changed = project_info(json!({"devCommand": "echo new"}), json!({}), false, true);
                    changed["fingerprint"] = json!("fp-new");
                    Ok(changed)
                }
            });
            let log: Log = Rc::default();
            open(
                &mut app,
                review(
                    project("fp-old", false, true),
                    Some(recorder(&log, "resume")),
                    ReviewSkip::Resume,
                ),
            );
            handle_review_project(&mut app, &enter());
            settle(&mut app, &mut rx).await;
            assert_eq!(app.messages.error.as_deref(), Some("repository configuration changed"));
            assert!(!app.messages.busy);
            assert_eq!(app.mode, Mode::ReviewProject);
            let shown = app
                .review
                .review
                .clone()
                .map(|review| review.project.project.fingerprint);
            assert_eq!(shown.as_deref(), Some("fp-new"));
            assert!(log.borrow().is_empty());
            let types: Vec<String> = seen
                .borrow()
                .iter()
                .map(|message| message["type"].as_str().unwrap_or_default().to_owned())
                .collect();
            assert_eq!(types, ["trust-project", "project-info"]);
        });
    }

    #[test]
    fn close_review_forgets_it_and_goes_back() {
        let (mut app, _rx) = app(AppProps::default());
        open(&mut app, review(project("x", false, true), None, ReviewSkip::Resume));
        close_review(&mut app);
        assert_eq!((app.mode, app.review.review.is_none()), (Mode::Browse, true));
        // Closing again is harmless.
        close_review(&mut app);
        assert_eq!(app.mode, Mode::Browse);
    }
}
