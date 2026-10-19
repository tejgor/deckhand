//! Port of app.tsx's workspace overview (i): the worktree's summary, P fetches its PR's state, b opens the PR, c
//! asks to push and open GitHub's PR form (only Enter on that confirmation pushes), g the Git tab.

use std::cell::RefCell;
use std::rc::Rc;

use deckhand_client::live::LiveClient;
use deckhand_core::display::error_message;
use deckhand_core::key::Input;
use deckhand_protocol::data::{CreatePrResult, RightPaneTab};
use deckhand_sys::desktop::open_url;

use crate::app::state::{App, Mode, WorkspaceInfo};
use crate::keymap::{Binding, HelpRef, KeySpec, Keymap, NamedKey};

/// workspace-info keys.
pub static KEYMAP: Keymap = &[
    Binding {
        keys: &[KeySpec::Char("c")],
        action: "ask to push the branch and open GitHub's PR form",
        help: HelpRef::Topic("Git & PRs"),
    },
    Binding {
        keys: &[KeySpec::Char("P")],
        action: "fetch the PR and check status",
        help: HelpRef::Topic("Git & PRs"),
    },
    Binding {
        keys: &[KeySpec::Char("b")],
        action: "open the PR in the browser",
        help: HelpRef::Topic("Git & PRs"),
    },
    Binding {
        keys: &[KeySpec::Char("g")],
        action: "open the Git tab",
        help: HelpRef::Topic("Git & PRs"),
    },
    Binding {
        keys: &[KeySpec::Named(NamedKey::Return)],
        action: "on the PR confirmation: push and open the PR form",
        help: HelpRef::Topic("Git & PRs"),
    },
    Binding {
        keys: &[KeySpec::Named(NamedKey::Escape)],
        action: "cancel the confirmation, or close the overview",
        help: HelpRef::Topic("Git & PRs"),
    },
];

/// TS `setWorkspaceInfo(current => current?.sessionId === sessionId ? {...current, ...} : current)`.
fn update_info(app: &mut App, session_id: &str, change: impl FnOnce(&mut WorkspaceInfo)) {
    if let Some(info) = app
        .workspace_info
        .info
        .as_mut()
        .filter(|info| info.session_id == session_id)
    {
        change(info);
    }
}

/// TS `++workspaceRequestRef.current`.
fn next_request(app: &mut App) -> u64 {
    app.workspace_info.request = app.workspace_info.request.wrapping_add(1);
    app.workspace_info.request
}

/// TS: i in browse: the overview of the selected session's workspace, its summary requested.
pub fn open_workspace_info(app: &mut App) {
    let (Some(client), Some(session)) = (app.client(), app.selected_session()) else {
        return;
    };
    let session_id = session.id;
    let request_id = next_request(app);
    app.workspace_info.info = Some(WorkspaceInfo {
        session_id: session_id.clone(),
        ..WorkspaceInfo::default()
    });
    app.review.details_scroll = 0;
    app.set_mode(Mode::WorkspaceInfo);
    let reply = client.workspace_summary(&session_id, None);
    let task = app.dispatch().spawn(reply, move |app: &mut App, result| match result {
        Ok(summary) => {
            if app.workspace_info.request == request_id {
                app.workspace_info.info = Some(WorkspaceInfo {
                    session_id,
                    summary: Some(summary),
                    ..WorkspaceInfo::default()
                });
            }
        }
        Err(error) => {
            if app.workspace_info.request == request_id {
                app.set_error(Some(error_message(&error)));
            }
        }
    });
    drop(task);
}

/// TS: the status line after pushing for a PR.
fn pushed_message(result: &CreatePrResult) -> String {
    if result.existing {
        format!("Pushed {} to {}; opened its PR", result.branch, result.remote)
    } else {
        let base = match result.base.as_ref() {
            Some(base) if !base.is_empty() => format!(" (base {base})"),
            _ => String::new(),
        };
        format!(
            "Pushed {} to {}; opened GitHub's new-PR form{base}",
            result.branch, result.remote
        )
    }
}

/// TS: Enter on the PR confirmation: push the branch and open GitHub's PR form, then refresh the counts.
fn create_pr(app: &mut App, client: Rc<LiveClient>, session_id: String, branch: String) {
    let reply = client.create_pr(&session_id, Some(&branch));
    let task = app.dispatch().spawn(reply, move |app: &mut App, result| {
        update_info(app, &session_id, |info| info.creating_pr = false);
        match result {
            Ok(result) => {
                app.set_status(Some(pushed_message(&result)));
                // Refresh the counts; P fetches the PR now that the cache was invalidated.
                let request_id = next_request(app);
                let reply = client.workspace_summary(&session_id, None);
                let refresh = app.dispatch().spawn(reply, move |app: &mut App, result| {
                    if let Ok(next) = result
                        && app.workspace_info.request == request_id
                    {
                        update_info(app, &session_id, |info| info.summary = Some(next));
                    }
                });
                drop(refresh);
            }
            Err(error) => app.set_error(Some(error_message(&error))),
        }
    });
    drop(task);
}

/// TS `currentWorkspaceInfo?.confirmPr`: pushing is outward-facing, so only Enter on this confirmation runs it.
fn handle_confirm_pr(app: &mut App, input: &Input, info: &WorkspaceInfo) {
    let key = &input.key;
    let session_id = info.session_id.clone();
    if key.escape {
        update_info(app, &session_id, |info| info.confirm_pr = false);
        return;
    }
    if app.scroll_details_pane(input) {
        return;
    }
    let (true, Some(client), Some(summary)) = (key.return_, app.client(), info.summary.as_ref()) else {
        return;
    };
    app.review.details_scroll = 0;
    app.set_error(None);
    update_info(app, &session_id, |info| {
        info.confirm_pr = false;
        info.creating_pr = true;
    });
    create_pr(app, client, session_id, summary.branch.clone());
}

/// TS: P: fetch the PR's state (`gh`), keeping the overview as it is meanwhile.
fn fetch_pr(app: &mut App, client: Rc<LiveClient>, session_id: String) {
    let request_id = next_request(app);
    update_info(app, &session_id, |info| info.pr_loading = true);
    let reply = client.workspace_summary(&session_id, Some(true));
    let task = app.dispatch().spawn(reply, move |app: &mut App, result| {
        if app.workspace_info.request != request_id {
            return;
        }
        match result {
            Ok(summary) => {
                app.workspace_info.info = Some(WorkspaceInfo {
                    session_id,
                    summary: Some(summary),
                    ..WorkspaceInfo::default()
                });
            }
            Err(error) => {
                update_info(app, &session_id, |info| info.pr_loading = false);
                app.set_error(Some(error_message(&error)));
            }
        }
    });
    drop(task);
}

/// TS: the workspace-info branch of the useTerminalInput handler.
pub fn handle_workspace_info(app: &mut App, input: &Input) {
    let key = &input.key;
    let info = app.current_workspace_info().cloned();
    if let Some(info) = info.as_ref().filter(|info| info.confirm_pr) {
        handle_confirm_pr(app, input, info);
        return;
    }
    if key.escape {
        app.set_mode(Mode::Browse);
        return;
    }
    if app.scroll_details_pane(input) {
        return;
    }
    let summary = info.as_ref().and_then(|info| info.summary.as_ref());
    let creating = info.as_ref().is_some_and(|info| info.creating_pr);
    match input.input.as_str() {
        "c" if !creating => {
            let (Some(info), Some(summary)) = (info.as_ref(), summary) else {
                return;
            };
            if summary.branch == "(detached)" {
                app.set_error(Some(
                    "HEAD is detached; check out a branch before creating a PR".to_owned(),
                ));
                return;
            }
            app.review.details_scroll = 0;
            update_info(app, &info.session_id, |info| info.confirm_pr = true);
        }
        "P" if !creating => {
            let (Some(client), Some(info)) = (app.client(), info.as_ref()) else {
                return;
            };
            if info.summary.is_some() && !info.pr_loading {
                fetch_pr(app, client, info.session_id.clone());
            }
        }
        "b" if summary.is_some_and(|summary| summary.pr.is_some()) => {
            let Some(pr) = summary.and_then(|summary| summary.pr.as_ref()) else {
                return;
            };
            let failed = RefCell::new(None);
            open_url(&pr.url, &|message| *failed.borrow_mut() = Some(message));
            if let Some(message) = failed.into_inner() {
                app.set_error(Some(message));
            }
        }
        "g" => {
            app.set_active_tab(RightPaneTab::Git);
            app.set_mode(Mode::Browse);
        }
        _ => {}
    }
}

/// A fake daemon for tests of the modes that call it: a `LiveClient` on a socket pair whose far end answers.
#[cfg(test)]
pub(crate) mod fake_daemon {
    use std::cell::RefCell;
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream as StdStream;
    use std::rc::Rc;

    use deckhand_client::live::{LiveClient, LiveClientHandlers};
    use serde_json::{Value, json};
    use tokio::sync::mpsc::UnboundedReceiver;

    use crate::app::event_loop::Update;
    use crate::app::state::App;

    /// Requests the fake daemon saw (type and the whole message), and how it answers each type.
    pub type Seen = Rc<RefCell<Vec<Value>>>;

    /// A LiveClient over a socket pair whose other end answers with `answer(message) -> Result<data, error>`.
    pub fn connect(app: &mut App, answer: impl Fn(&Value) -> Result<Value, String> + Send + 'static) -> Seen {
        let (ours, theirs) = StdStream::pair().unwrap_or_else(|error| panic!("{error}"));
        let seen: Seen = Rc::default();
        let log = Rc::clone(&seen);
        let (lines_tx, mut lines_rx) = tokio::sync::mpsc::unbounded_channel::<Value>();
        let writer = theirs.try_clone().unwrap_or_else(|error| panic!("{error}"));
        std::thread::spawn(move || {
            let mut writer = writer;
            for line in BufReader::new(theirs).lines().map_while(Result::ok) {
                let Ok(message) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                let reply = match answer(&message) {
                    Ok(data) => {
                        json!({"type": "response", "requestId": message["requestId"], "ok": true, "data": data})
                    }
                    Err(error) => {
                        json!({"type": "response", "requestId": message["requestId"], "ok": false, "error": error})
                    }
                };
                if lines_tx.send(message).is_err() || writeln!(writer, "{reply}").is_err() {
                    break;
                }
            }
        });
        tokio::task::spawn_local(async move {
            while let Some(message) = lines_rx.recv().await {
                log.borrow_mut().push(message);
            }
        });
        ours.set_nonblocking(true).unwrap_or_else(|error| panic!("{error}"));
        let socket = tokio::net::UnixStream::from_std(ours).unwrap_or_else(|error| panic!("{error}"));
        app.connection.client = Some(Rc::new(LiveClient::new(socket, LiveClientHandlers::default())));
        seen
    }

    /// Runs the model updates that arrive within a short quiet period.
    pub async fn settle(app: &mut App, rx: &mut UnboundedReceiver<Update<App>>) {
        while let Ok(Some(update)) = tokio::time::timeout(std::time::Duration::from_millis(300), rx.recv()).await {
            update(app);
        }
    }

    /// A repository config as the daemon reports it (`config`: what deckhand.json holds, `effective`: what applies).
    pub fn project_info(config: Value, effective: Value, trusted: bool, needs_review: bool) -> Value {
        json!({
            "root": "/repo", "trustRoot": "/repo", "path": "/repo/deckhand.json", "config": config, "exists": true,
            "fingerprint": "fp1", "trusted": trusted, "needsReview": needs_review, "effective": effective
        })
    }

    pub fn local(body: impl std::future::Future<Output = ()>) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap_or_else(|error| panic!("{error}"));
        tokio::task::LocalSet::new().block_on(&runtime, body);
    }
}

#[cfg(test)]
mod tests {
    use super::fake_daemon::{connect, local, settle};
    use super::*;
    use crate::app::event_loop::Update;
    use crate::app::state::AppProps;
    use crate::app::test_support::{app, session};
    use deckhand_core::key::Key;
    use deckhand_protocol::data::SessionStatus;
    use serde_json::{Value, json};
    use tokio::sync::mpsc::UnboundedReceiver;

    fn summary(branch: &str) -> Value {
        json!({
            "cwd": "/repo", "branch": branch, "head": "abc1234", "changedFiles": 2, "untrackedFiles": 1,
            "additions": 5, "deletions": 3
        })
    }

    fn with_pr() -> Value {
        let mut value = summary("feature");
        value["pr"] = json!({
            "number": 7, "url": "http://example.invalid/pull/7", "state": "OPEN",
            "checks": "passing"
        });
        value
    }

    fn typed(text: &str) -> Input {
        Input::text(text)
    }

    fn pressed(set: impl FnOnce(&mut Key)) -> Input {
        let mut key = Key::NONE;
        set(&mut key);
        Input::key("", key)
    }

    fn selected_app() -> (App, UnboundedReceiver<Update<App>>) {
        let (mut app, rx) = app(AppProps::default());
        app.data.repo_sessions = vec![session("a", "Alpha", SessionStatus::Running)];
        app.set_selected_id(Some("a".to_owned()));
        (app, rx)
    }

    #[test]
    fn i_requests_the_summary_and_shows_it() {
        local(async {
            let (mut app, mut rx) = selected_app();
            let seen = connect(&mut app, |message| {
                assert_eq!(message["type"], "workspace-summary");
                Ok(summary("feature"))
            });
            open_workspace_info(&mut app);
            assert_eq!(app.mode, Mode::WorkspaceInfo);
            assert_eq!(
                app.workspace_info.info.as_ref().map(|info| info.summary.is_none()),
                Some(true)
            );
            assert_eq!(app.review.details_scroll, 0);
            settle(&mut app, &mut rx).await;
            let info = app.workspace_info.info.clone().unwrap_or_default();
            assert_eq!(info.session_id, "a");
            assert_eq!(info.summary.map(|summary| summary.branch), Some("feature".to_owned()));
            let seen = seen.borrow();
            assert_eq!(seen[0]["sessionId"], "a");
            assert_eq!(seen[0]["includePr"], false);
        });
    }

    #[test]
    fn i_does_nothing_without_a_client_or_a_selected_session() {
        local(async {
            let (mut app, _rx) = app(AppProps::default());
            open_workspace_info(&mut app);
            assert_eq!((app.mode, app.workspace_info.info.is_none()), (Mode::Browse, true));
            let (mut app, _rx) = selected_app();
            open_workspace_info(&mut app);
            assert_eq!(app.mode, Mode::Browse);
        });
    }

    #[test]
    fn a_failed_summary_is_an_error_and_a_late_one_is_ignored() {
        local(async {
            let (mut app, mut rx) = selected_app();
            let _seen = connect(&mut app, |_| Err("no such workspace".to_owned()));
            open_workspace_info(&mut app);
            settle(&mut app, &mut rx).await;
            assert_eq!(app.messages.error.as_deref(), Some("no such workspace"));
            assert_eq!(app.mode, Mode::WorkspaceInfo);
            // A newer request makes the older answer stale.
            app.messages.error = None;
            let _seen = connect(&mut app, |_| Ok(summary("late")));
            open_workspace_info(&mut app);
            app.workspace_info.request += 1;
            settle(&mut app, &mut rx).await;
            assert!(
                app.workspace_info
                    .info
                    .as_ref()
                    .is_some_and(|info| info.summary.is_none())
            );
        });
    }

    #[test]
    fn escape_closes_and_g_opens_the_git_tab() {
        let (mut app, _rx) = selected_app();
        app.workspace_info.info = Some(WorkspaceInfo {
            session_id: "a".to_owned(),
            ..WorkspaceInfo::default()
        });
        app.set_mode(Mode::WorkspaceInfo);
        handle_workspace_info(&mut app, &pressed(|key| key.escape = true));
        assert_eq!(app.mode, Mode::Browse);
        app.set_mode(Mode::WorkspaceInfo);
        handle_workspace_info(&mut app, &typed("g"));
        assert_eq!((app.mode, app.browse.active_tab), (Mode::Browse, RightPaneTab::Git));
    }

    #[test]
    fn j_scrolls_the_overview_before_other_keys_apply() {
        let (mut app, _rx) = selected_app();
        // A short terminal leaves the overview more lines than rows.
        app.browse.terminal_size = crate::app::state::TerminalSize { cols: 40, rows: 12 };
        overview(&mut app, summary("feature"));
        handle_workspace_info(&mut app, &typed("j"));
        assert_eq!(app.mode, Mode::WorkspaceInfo);
        assert!(app.review.details_scroll > 0);
        // Asking for the PR starts the confirmation from the top.
        handle_workspace_info(&mut app, &typed("c"));
        assert_eq!(app.review.details_scroll, 0);
    }

    fn overview(app: &mut App, summary_json: Value) {
        app.workspace_info.info = Some(WorkspaceInfo {
            session_id: "a".to_owned(),
            summary: serde_json::from_value(summary_json).ok(),
            ..WorkspaceInfo::default()
        });
        app.set_mode(Mode::WorkspaceInfo);
    }

    #[test]
    fn c_asks_first_and_a_detached_head_cannot_create_a_pr() {
        let (mut app, _rx) = selected_app();
        overview(&mut app, summary("(detached)"));
        handle_workspace_info(&mut app, &typed("c"));
        assert_eq!(
            app.messages.error.as_deref(),
            Some("HEAD is detached; check out a branch before creating a PR")
        );
        assert!(app.workspace_info.info.as_ref().is_some_and(|info| !info.confirm_pr));
        overview(&mut app, summary("feature"));
        app.review.details_scroll = 3;
        handle_workspace_info(&mut app, &typed("c"));
        assert!(app.workspace_info.info.as_ref().is_some_and(|info| info.confirm_pr));
        assert_eq!(app.review.details_scroll, 0);
        // Esc cancels the confirmation only.
        handle_workspace_info(&mut app, &pressed(|key| key.escape = true));
        assert_eq!(app.mode, Mode::WorkspaceInfo);
        assert!(app.workspace_info.info.as_ref().is_some_and(|info| !info.confirm_pr));
        // Without a summary there is nothing to create.
        app.workspace_info.info = Some(WorkspaceInfo {
            session_id: "a".to_owned(),
            ..WorkspaceInfo::default()
        });
        handle_workspace_info(&mut app, &typed("c"));
        assert!(app.workspace_info.info.as_ref().is_some_and(|info| !info.confirm_pr));
    }

    #[test]
    fn only_enter_on_the_confirmation_pushes() {
        local(async {
            let (mut app, mut rx) = selected_app();
            let seen = connect(&mut app, |message| match message["type"].as_str() {
                Some("create-pr") => {
                    Ok(json!({"branch": "feature", "remote": "origin", "base": "main", "existing": false}))
                }
                _ => Ok(summary("feature")),
            });
            overview(&mut app, summary("feature"));
            handle_workspace_info(&mut app, &typed("c"));
            // Other keys do nothing on the confirmation.
            handle_workspace_info(&mut app, &typed("x"));
            handle_workspace_info(&mut app, &typed("P"));
            settle(&mut app, &mut rx).await;
            assert!(seen.borrow().is_empty());
            app.messages.error = Some("old".to_owned());
            handle_workspace_info(&mut app, &pressed(|key| key.return_ = true));
            let info = app.workspace_info.info.clone().unwrap_or_default();
            assert!(!info.confirm_pr && info.creating_pr);
            assert_eq!(app.messages.error, None);
            settle(&mut app, &mut rx).await;
            let info = app.workspace_info.info.clone().unwrap_or_default();
            assert!(!info.creating_pr && info.summary.is_some());
            assert_eq!(
                app.messages.status_message.as_deref(),
                Some("Pushed feature to origin; opened GitHub's new-PR form (base main)")
            );
            let types: Vec<String> = seen
                .borrow()
                .iter()
                .map(|message| message["type"].as_str().unwrap_or_default().to_owned())
                .collect();
            assert_eq!(types, ["create-pr", "workspace-summary"]);
            assert_eq!(seen.borrow()[0]["branch"], "feature");
        });
    }

    #[test]
    fn an_existing_pr_and_a_failed_push_are_reported() {
        local(async {
            let (mut app, mut rx) = selected_app();
            let _seen = connect(&mut app, |message| match message["type"].as_str() {
                Some("create-pr") => Ok(json!({"branch": "feature", "remote": "origin", "existing": true})),
                _ => Ok(summary("feature")),
            });
            overview(&mut app, summary("feature"));
            handle_workspace_info(&mut app, &typed("c"));
            handle_workspace_info(&mut app, &pressed(|key| key.return_ = true));
            settle(&mut app, &mut rx).await;
            assert_eq!(
                app.messages.status_message.as_deref(),
                Some("Pushed feature to origin; opened its PR")
            );
            let _seen = connect(&mut app, |_| Err("push rejected".to_owned()));
            overview(&mut app, summary("feature"));
            handle_workspace_info(&mut app, &typed("c"));
            handle_workspace_info(&mut app, &pressed(|key| key.return_ = true));
            settle(&mut app, &mut rx).await;
            assert_eq!(app.messages.error.as_deref(), Some("push rejected"));
            assert!(
                app.workspace_info
                    .info
                    .as_ref()
                    .is_some_and(|info| !info.creating_pr && info.summary.is_some())
            );
        });
    }

    #[test]
    fn p_fetches_the_pr_state_once_at_a_time() {
        local(async {
            let (mut app, mut rx) = selected_app();
            let seen = connect(&mut app, |_| Ok(with_pr()));
            overview(&mut app, summary("feature"));
            handle_workspace_info(&mut app, &typed("P"));
            assert!(app.workspace_info.info.as_ref().is_some_and(|info| info.pr_loading));
            // Already loading: a second P sends nothing.
            handle_workspace_info(&mut app, &typed("P"));
            settle(&mut app, &mut rx).await;
            let info = app.workspace_info.info.clone().unwrap_or_default();
            assert!(!info.pr_loading);
            assert_eq!(info.summary.and_then(|summary| summary.pr).map(|pr| pr.number), Some(7));
            assert_eq!(seen.borrow().len(), 1);
            assert_eq!(seen.borrow()[0]["includePr"], true);
        });
    }

    #[test]
    fn a_failed_pr_fetch_stops_loading_and_reports() {
        local(async {
            let (mut app, mut rx) = selected_app();
            let _seen = connect(&mut app, |_| Err("gh is not installed".to_owned()));
            overview(&mut app, summary("feature"));
            handle_workspace_info(&mut app, &typed("P"));
            settle(&mut app, &mut rx).await;
            assert_eq!(app.messages.error.as_deref(), Some("gh is not installed"));
            assert!(
                app.workspace_info
                    .info
                    .as_ref()
                    .is_some_and(|info| !info.pr_loading && info.summary.is_some())
            );
            // Without a summary P does nothing.
            app.workspace_info.info = Some(WorkspaceInfo {
                session_id: "a".to_owned(),
                ..WorkspaceInfo::default()
            });
            handle_workspace_info(&mut app, &typed("P"));
            assert!(app.workspace_info.info.as_ref().is_some_and(|info| !info.pr_loading));
        });
    }

    #[test]
    fn b_opens_only_https_pr_links() {
        let (mut app, _rx) = selected_app();
        overview(&mut app, with_pr());
        handle_workspace_info(&mut app, &typed("b"));
        // The fixture's link is http: refused before anything starts.
        assert_eq!(app.messages.error.as_deref(), Some("refusing to open a non-https URL"));
        app.messages.error = None;
        overview(&mut app, summary("feature"));
        handle_workspace_info(&mut app, &typed("b"));
        assert_eq!(app.messages.error, None);
    }

    #[test]
    fn an_overview_of_another_session_is_not_current() {
        let (mut app, _rx) = selected_app();
        overview(&mut app, summary("feature"));
        app.workspace_info.info = app.workspace_info.info.clone().map(|info| WorkspaceInfo {
            session_id: "other".to_owned(),
            ..info
        });
        handle_workspace_info(&mut app, &typed("c"));
        assert_eq!(app.mode, Mode::WorkspaceInfo);
        assert!(app.workspace_info.info.as_ref().is_some_and(|info| !info.confirm_pr));
    }
}
