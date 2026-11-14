//! The plan confirmation (confirm-plan): an orchestrator's plan (`deckhand plan apply`) creates nothing until it is
//! answered here. The daemon pushes each waiting plan (`plan-pending`, again after every `subscribe`) and says when
//! one is settled (`plan-settled`: answered by any Deckhand, or expired).
//!
//! - **One at a time, oldest first.** Plans queue; `enter` creates the shown one, `esc` cancels it
//!   (`plan-decide`), and the next one follows. No other key acts while it shows.
//! - **Never over your typing.** It takes over the session list and typing to an agent or a shell (giving the typing
//!   back after the answer). On any other screen (a form, Tasks, Settings, another dialog) it waits until you leave
//!   it, while the header and the footer say a plan is waiting. Keys typed on as it opens are not answers: it takes
//!   them only after a pause ([`QUIET_MS`]).
//! - **Attached.** An attach ends when a plan arrives (`deckhand_client::attach::AttachEnd::PlanPending`) and the UI
//!   comes back with the attach in [`PlanState::reattach`]: once the last plan is settled (by you, another Deckhand
//!   or expiry) it attaches again. A key for anything else, or no plan to ask about within [`REATTACH_WAIT`] (it was
//!   settled before the UI came back), leaves you in Deckhand with that session selected.

use std::time::Duration;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use deckhand_core::display::{HintPart, error_message, fit_hint};
use deckhand_core::jsstr::date::now_ms;
use deckhand_core::key::Input;
use deckhand_core::plan::{entry_line, expiry_text};
use deckhand_protocol::data::{AttachTarget, PendingPlan, PlanDecision, RightPaneTab, SessionStatus, UiExitResult};
use deckhand_protocol::events::PlanSettled;
use deckhand_sys::async_util::Timer;

use crate::app::modes::{browse, preview_focus};
use crate::app::state::{App, DialogState, Mode};
use crate::keymap::{Binding, HelpRef, KeySpec, Keymap, NamedKey};
use crate::primitives::{
    Border, DialogRows, DialogScroll, DialogView, Row, Seg, TextStyle, Wrap, draw_dialog, fit_row,
};
use crate::theme::THEME;

/// confirm-plan keys.
pub static KEYMAP: Keymap = &[
    Binding {
        keys: &[KeySpec::Named(NamedKey::Return)],
        action: "create the plan",
        help: HelpRef::Hidden("the dialog shows its own hint line"),
    },
    Binding {
        keys: &[KeySpec::Named(NamedKey::Escape)],
        action: "cancel the plan",
        help: HelpRef::Hidden("the dialog shows its own hint line"),
    },
    crate::keymap::SCROLL_DIALOG,
];

/// Keys within this long of the dialog opening, or of the last key it ignored, are not taken as answers: a sentence
/// being typed to the agent (an `enter` in it) must not create the plan.
pub const QUIET_MS: i64 = 750;

/// How long a UI that came back for a plan waits for it before it stops meaning to attach again.
pub const REATTACH_WAIT: Duration = Duration::from_secs(10);

/// The plans waiting for an answer and the dialog's state.
#[derive(Debug, Default)]
pub struct PlanState {
    /// Every plan waiting, oldest first; the dialog shows the first.
    pub queue: Vec<PendingPlan>,
    /// The answer sent for the shown plan (`true`: create), until the daemon settles it.
    pub answering: Option<bool>,
    /// Why the last answer failed.
    pub error: Option<String>,
    /// When the dialog opened, or the last key it held back came (epoch milliseconds): see [`QUIET_MS`].
    pub held_since: Option<i64>,
    /// The typing the dialog took over (preview or terminal focus) in that session, given back after the answer.
    pub refocus: Option<(Mode, String)>,
    /// The attach a plan ended: attached again after the answer.
    pub reattach: Option<UiExitResult>,
    /// Forgets `reattach` when no plan came to ask about ([`REATTACH_WAIT`]).
    pub reattach_timer: Option<Timer>,
}

/// On mount, with an attach to go back to: forgotten unless a plan is waiting by [`REATTACH_WAIT`].
pub fn arm_reattach(app: &mut App) {
    if app.plans.reattach.is_none() {
        return;
    }
    app.plans.reattach_timer = Some(app.dispatch().after(REATTACH_WAIT, |app: &mut App| {
        app.plans.reattach_timer = None;
        if app.plans.queue.is_empty() {
            app.plans.reattach = None;
        }
    }));
}

/// `plan-pending`: queues the plan (oldest first; one already queued is replaced) and shows it if it can.
pub fn plan_pending(app: &mut App, plan: PendingPlan) {
    let shown = shown_id(app);
    let queue = &mut app.plans.queue;
    match queue.iter_mut().find(|queued| queued.id == plan.id) {
        Some(queued) => *queued = plan,
        None => queue.push(plan),
    }
    // ISO timestamps sort as text.
    queue.sort_by(|a, b| a.created_at.cmp(&b.created_at).then_with(|| a.id.cmp(&b.id)));
    if shown.is_some() && shown != shown_id(app) {
        // An older plan came first (after a reconnect): it shows from the top, answered by nobody yet.
        show_next(app);
    }
    open_if_free(app);
}

/// `plan-settled`: the plan is no longer waiting (answered by any Deckhand, or expired). The dialog moves on.
pub fn plan_settled(app: &mut App, settled: &PlanSettled) {
    if shown_id(app).as_deref() == Some(settled.plan_id.as_str()) {
        close_shown(app, settled.decision);
        return;
    }
    app.plans.queue.retain(|plan| plan.id != settled.plan_id);
}

/// The connection to the daemon went: what was waiting is sent again on the next `subscribe`, and what was settled
/// meanwhile is not, so the queue starts over.
pub fn connection_lost(app: &mut App) {
    app.plans.queue.clear();
    app.plans.answering = None;
    app.plans.error = None;
    app.plans.refocus = None;
    if app.mode == Mode::ConfirmPlan {
        app.set_mode(Mode::Browse);
    }
}

/// The plan the dialog shows.
fn shown_id(app: &App) -> Option<String> {
    (app.mode == Mode::ConfirmPlan)
        .then(|| app.plans.queue.first().map(|plan| plan.id.clone()))
        .flatten()
}

/// Whether a plan waits that the dialog does not show (the header and footer say so).
pub fn plan_waiting_hidden(app: &App) -> bool {
    !app.plans.queue.is_empty() && app.mode != Mode::ConfirmPlan
}

/// Shows the first waiting plan when nothing would be lost: from the session list, or typing to an agent or a shell
/// (left, and given back after the answer). Runs as an effect too, so a plan that waited opens as you leave a screen.
pub fn open_if_free(app: &mut App) {
    if app.plans.queue.is_empty() || app.mode == Mode::ConfirmPlan || app.messages.busy || app.connection.exit.is_some()
    {
        return;
    }
    match app.mode {
        Mode::Browse => {}
        Mode::PreviewFocus | Mode::TerminalFocus => {
            app.plans.refocus = app
                .preview_focus
                .session_id
                .clone()
                .map(|session_id| (app.mode, session_id));
            preview_focus::leave_focus(app, None);
        }
        _ => return,
    }
    app.set_mode(Mode::ConfirmPlan);
    show_next(app);
}

/// The dialog starts over for the plan now first: no answer, no error, at the top, keys quiet for a moment.
fn show_next(app: &mut App) {
    app.plans.answering = None;
    app.plans.error = None;
    app.plans.held_since = Some(now_ms());
    app.dialog = DialogState::default();
}

/// The shown plan was settled (`decision`): the next one shows, or the dialog closes, saying how it ended, and what
/// the plans interrupted comes back.
fn close_shown(app: &mut App, decision: PlanDecision) {
    if app.plans.queue.is_empty() {
        return;
    }
    let plan = app.plans.queue.remove(0);
    let ours = app.plans.answering.is_some();
    app.set_status(Some(settled_text(&plan, decision, ours)));
    if !app.plans.queue.is_empty() {
        show_next(app);
        return;
    }
    app.plans.answering = None;
    app.plans.error = None;
    app.set_mode(Mode::Browse);
    let refocus = app.plans.refocus.take();
    let reattach = app.plans.reattach.take();
    give_back(app, refocus, reattach);
}

/// What the status line says once a plan is settled.
fn settled_text(plan: &PendingPlan, decision: PlanDecision, ours: bool) -> String {
    match (decision, ours) {
        (PlanDecision::Approved, true) => format!("Plan approved: creating {}", plan.summary.counts),
        (PlanDecision::Rejected, true) => "Plan cancelled: nothing was created".to_owned(),
        (PlanDecision::Approved, false) => "The plan was approved in another Deckhand".to_owned(),
        (PlanDecision::Rejected, false) => "The plan was cancelled in another Deckhand".to_owned(),
        (PlanDecision::Expired, _) => "The plan expired unanswered: nothing was created".to_owned(),
    }
}

/// After the plans: attach again to what the plan detached from, or type again where it interrupted, while that is
/// still there (the agent running, for an attach to it or typing to it). Otherwise you stay on the session list.
fn give_back(app: &mut App, refocus: Option<(Mode, String)>, reattach: Option<UiExitResult>) {
    let available = |app: &App, id: &str, needs_agent: bool| {
        app.data
            .repo_sessions
            .iter()
            .any(|session| session.id == id && (!needs_agent || session.status == SessionStatus::Running))
    };
    if let Some(attach) = reattach {
        if let UiExitResult::Attach { session_id, target, .. } = &attach
            && available(app, session_id, *target == AttachTarget::Agent)
        {
            app.exit_with(attach);
        }
        return;
    }
    let tab = match refocus.as_ref().map(|(mode, _)| *mode) {
        Some(Mode::TerminalFocus) => RightPaneTab::Terminal,
        _ => RightPaneTab::Preview,
    };
    if let Some((mode, session_id)) = refocus
        && app.browse.active_tab == tab
        && app.selected_id_ref().as_deref() == Some(session_id.as_str())
        && available(app, &session_id, mode == Mode::PreviewFocus)
    {
        browse::step_into_pane(app);
    }
}

/// confirm-plan's keys: `enter` creates the plan, `esc` cancels it; J/K scroll (in `App::handle_input`).
pub fn handle_confirm_plan(app: &mut App, input: &Input) {
    handle_confirm_plan_at(app, input, now_ms());
}

/// [`handle_confirm_plan`] at `now` (epoch milliseconds).
pub fn handle_confirm_plan_at(app: &mut App, input: &Input, now: i64) {
    if let Some(since) = app.plans.held_since {
        if now - since < QUIET_MS {
            app.plans.held_since = Some(now);
            return;
        }
        app.plans.held_since = None;
    }
    if app.plans.answering.is_some() {
        return;
    }
    if input.key.return_ {
        answer(app, true);
    } else if input.key.escape {
        answer(app, false);
    }
}

/// Sends the answer for the shown plan. The dialog closes when the daemon settles it (or answers this request);
/// a refusal shows in the dialog.
fn answer(app: &mut App, approve: bool) {
    let Some(plan_id) = app.plans.queue.first().map(|plan| plan.id.clone()) else {
        return;
    };
    let Some(client) = app.client() else {
        app.plans.error = Some("still connecting to daemon".to_owned());
        return;
    };
    app.plans.answering = Some(approve);
    app.plans.error = None;
    let reply = client.plan_decide(&plan_id, approve);
    drop(app.dispatch().spawn(reply, move |app: &mut App, result| {
        let shown = shown_id(app).as_deref() == Some(plan_id.as_str());
        match result {
            Ok(()) if shown => close_shown(
                app,
                if approve {
                    PlanDecision::Approved
                } else {
                    PlanDecision::Rejected
                },
            ),
            Ok(()) => app.plans.queue.retain(|plan| plan.id != plan_id),
            Err(error) if shown => {
                app.plans.answering = None;
                app.plans.error = Some(error_message(&error));
            }
            Err(error) => app.set_error(Some(error_message(&error))),
        }
    }));
}

/// The dialog's props.
#[derive(Clone, Copy, Debug)]
pub struct PlanDialogProps<'a> {
    pub plan: &'a PendingPlan,
    /// Plans waiting after this one.
    pub more: usize,
    pub answering: Option<bool>,
    pub error: Option<&'a str>,
    pub now_ms: i64,
}

fn text(text: impl Into<String>, style: TextStyle) -> Row {
    vec![Seg::new(text, style)]
}

/// How many lines of the idea's brief the dialog shows.
const BRIEF_ROWS: usize = 4;

/// The dialog `width` wide: what the plan holds (and the idea it is for), its tree of sessions, its parked work (or
/// backlog tasks), the idea's brief and when it expires; the answer keys (or the answer being sent) at the bottom.
pub fn plan_dialog_rows(props: &PlanDialogProps<'_>, width: u16) -> DialogRows {
    let content_width = usize::from(width).saturating_sub(4).max(1);
    let muted = TextStyle::fg(THEME.muted);
    let plan = props.plan;
    let summary = &plan.summary;
    let mut rows = DialogRows::default();
    rows.header.extend(fit_row(
        &text("Create this plan?", TextStyle::fg(THEME.accent).bold(true)),
        content_width,
        Wrap::Wrap,
    ));
    let idea = summary.idea.as_deref().filter(|idea| !idea.is_empty());
    let from = match (idea, plan.caller_title.as_deref().filter(|title| !title.is_empty())) {
        (Some(idea), _) => format!("A plan for ◎ {idea}: {}.", summary.counts),
        (None, Some(caller)) => format!("{caller}'s plan: {}.", summary.counts),
        (None, None) => format!("A plan of {}.", summary.counts),
    };
    let explanation = match (summary.sessions.is_empty(), idea.is_some()) {
        (true, true) => format!("{from} Its tasks are parked under the idea."),
        (true, false) => format!("{from} Its tasks go to the backlog."),
        (false, true) => format!("{from} Each session starts on its brief at once, under the orchestrator."),
        (false, false) => format!("{from} Each session starts on its brief at once."),
    };
    rows.body
        .extend(fit_row(&text(explanation, muted), content_width, Wrap::Wrap));
    if !summary.sessions.is_empty() {
        rows.body.push(Vec::new());
    }
    for entry in &summary.sessions {
        let line = entry_line(entry);
        let row = vec![
            Seg::new(
                format!("{}{} ", " ".repeat(line.indent), line.marker),
                TextStyle::fg(THEME.muted),
            ),
            Seg::new(line.title, TextStyle::fg(THEME.accent_soft)),
            Seg::new(format!("  ({})", line.detail), muted),
        ];
        rows.body.extend(fit_row(&row, content_width, Wrap::Wrap));
    }
    if !summary.tasks.is_empty() {
        rows.body.push(Vec::new());
        let heading = if idea.is_some() {
            "Parked under the idea"
        } else {
            "Backlog"
        };
        rows.body.push(text(heading, TextStyle::fg(THEME.accent).bold(true)));
        for task in &summary.tasks {
            rows.body.extend(fit_row(
                &text(format!("☐ {}", task.title), TextStyle::fg(THEME.accent_soft)),
                content_width,
                Wrap::Wrap,
            ));
            if let Some(details) = task.details.as_deref().filter(|details| !details.trim().is_empty()) {
                let details = details.split_whitespace().collect::<Vec<_>>().join(" ");
                rows.body
                    .extend(fit_row(&text(format!("  {details}"), muted), content_width, Wrap::Wrap));
            }
        }
    }
    if let Some(brief) = summary.brief.as_deref().filter(|brief| !brief.trim().is_empty()) {
        rows.body.push(Vec::new());
        rows.body
            .push(text("The idea's brief", TextStyle::fg(THEME.accent).bold(true)));
        let lines: Vec<&str> = brief.trim_end().lines().collect();
        for line in lines.iter().take(BRIEF_ROWS) {
            rows.body
                .extend(fit_row(&text(*line, muted), content_width, Wrap::Wrap));
        }
        if lines.len() > BRIEF_ROWS {
            let more = lines.len() - BRIEF_ROWS;
            rows.body.push(text(
                format!("… {more} more {}", if more == 1 { "line" } else { "lines" }),
                muted,
            ));
        }
    }
    rows.footer.push(Vec::new());
    let mut waits = Vec::new();
    if let Some(expiry) = expiry_text(&plan.expires_at, props.now_ms) {
        waits.push(format!("Unanswered, it {expiry}."));
    }
    if props.more > 0 {
        let plans = if props.more == 1 { "plan" } else { "plans" };
        waits.push(format!("{} more {plans} waiting.", props.more));
    }
    if !waits.is_empty() {
        rows.footer
            .extend(fit_row(&text(waits.join(" "), muted), content_width, Wrap::Wrap));
    }
    if let Some(error) = props.error.filter(|error| !error.is_empty()) {
        rows.footer.extend(fit_row(
            &text(error, TextStyle::fg(THEME.error)),
            content_width,
            Wrap::Wrap,
        ));
    }
    let hint = match props.answering {
        Some(true) => "Creating…".to_owned(),
        Some(false) => "Cancelling…".to_owned(),
        None => {
            let parts: Vec<HintPart> = vec!["enter create".into(), "esc cancel".into()];
            fit_hint(&parts, content_width, " • ")
        }
    };
    rows.footer.push(text(hint, muted));
    rows
}

/// [`plan_dialog_rows`] drawn over `area` as `view` keeps it.
pub fn render_plan_dialog(props: &PlanDialogProps<'_>, buf: &mut Buffer, area: Rect, view: DialogView) -> DialogScroll {
    let build = |width| plan_dialog_rows(props, width);
    draw_dialog(buf, area, &Border::round(THEME.border_active), &build, view)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use deckhand_core::key::Key;
    use deckhand_protocol::data::PlanDecision;

    use super::*;
    use crate::app::layout::{PLAN_WAITING_HEADER, PLAN_WAITING_MESSAGE, footer_messages, header_rows};
    use crate::app::modes::browse::tests::{Daemon, apply, local, named, text};
    use crate::app::state::AppProps;
    use crate::app::test_support::{app, session};
    use crate::primitives::{row_text, string_width};

    /// A plan of Auth rework (with Token storage under it) and a script, with one backlog task; created `minute`
    /// minutes past noon, waiting 15 minutes.
    fn pending(id: &str, minute: u32) -> PendingPlan {
        serde_json::from_value(json!({
            "id": id,
            "repoRoot": "/repo",
            "callerSessionId": "orch",
            "callerTitle": "Orchestrator",
            "plan": {"sessions": [], "tasks": []},
            "summary": {
                "sessions": [
                    {"key": "auth", "title": "Auth rework", "depth": 0, "agent": "claude", "worktree": "new"},
                    {"key": "tokens", "title": "Token storage", "depth": 1, "agent": "claude", "worktree": "new",
                     "parentKey": "auth", "stacked": true, "stackedOnTitle": "Auth rework"},
                    {"key": "script", "title": "Migration script", "depth": 0, "agent": "codex", "worktree": "none"},
                ],
                "tasks": [{"title": "Remove legacy endpoint", "details": "After Auth rework merges:\nthe old routes go."}],
                "counts": "3 sessions, 1 task",
            },
            "createdAt": format!("2026-11-14T12:{minute:02}:00.000Z"),
            "expiresAt": format!("2026-11-14T12:{:02}:00.000Z", minute + 15),
        }))
        .unwrap_or_else(|error| panic!("{error}"))
    }

    fn settled(id: &str, decision: PlanDecision) -> PlanSettled {
        PlanSettled {
            plan_id: id.to_owned(),
            decision,
        }
    }

    fn enter() -> Input {
        named(Key {
            return_: true,
            ..Key::NONE
        })
    }

    fn esc() -> Input {
        named(Key {
            escape: true,
            ..Key::NONE
        })
    }

    /// A key after the dialog's quiet moment.
    fn press(app: &mut App, input: &Input) {
        let later = app.plans.held_since.map_or_else(now_ms, |since| since + QUIET_MS);
        handle_confirm_plan_at(app, input, later);
    }

    fn shown(app: &App) -> Option<&str> {
        (app.mode == Mode::ConfirmPlan)
            .then(|| app.plans.queue.first().map(|plan| plan.id.as_str()))
            .flatten()
    }

    #[test]
    fn a_waiting_plan_opens_the_dialog_and_enter_creates_it() {
        local(async {
            let (mut app, mut rx) = app(AppProps::default());
            let mut daemon = Daemon::connect(&mut app);
            plan_pending(&mut app, pending("p1", 0));
            assert_eq!(shown(&app), Some("p1"));
            // Keys typed on as it opens are not answers; nor is anything but enter and esc.
            let opened = app.plans.held_since.unwrap_or_default();
            handle_confirm_plan_at(&mut app, &enter(), opened + 100);
            handle_confirm_plan_at(&mut app, &enter(), opened + 100 + QUIET_MS - 1);
            press(&mut app, &text("x"));
            assert_eq!(app.plans.answering, None);
            press(&mut app, &enter());
            assert_eq!(app.plans.answering, Some(true));
            let decide = daemon.next().await;
            assert_eq!(decide["type"], "plan-decide");
            assert_eq!(decide["planId"], "p1");
            assert_eq!(decide["approve"], true);
            // While the answer is on its way, keys do nothing more.
            press(&mut app, &esc());
            daemon.ok(&decide, json!({"ok": true})).await;
            apply(&mut app, &mut rx).await;
            assert_eq!(app.mode, Mode::Browse);
            assert!(app.plans.queue.is_empty());
            assert_eq!(
                app.messages.status_message.as_deref(),
                Some("Plan approved: creating 3 sessions, 1 task")
            );
            // Its plan-settled, arriving after, changes nothing.
            plan_settled(&mut app, &settled("p1", PlanDecision::Approved));
            assert_eq!(app.mode, Mode::Browse);
        });
    }

    #[test]
    fn esc_cancels_and_a_refusal_shows_in_the_dialog_until_it_is_settled() {
        local(async {
            let (mut app, mut rx) = app(AppProps::default());
            let mut daemon = Daemon::connect(&mut app);
            plan_pending(&mut app, pending("p1", 0));
            press(&mut app, &esc());
            let decide = daemon.next().await;
            assert_eq!(decide["approve"], false);
            daemon
                .fail(
                    &decide,
                    "No plan p1 is waiting for an answer (it was answered or expired)",
                )
                .await;
            apply(&mut app, &mut rx).await;
            assert_eq!(shown(&app), Some("p1"));
            assert_eq!(app.plans.answering, None);
            let error = app.plans.error.clone().unwrap_or_default();
            assert!(error.starts_with("No plan p1 is waiting"), "{error}");
            let rows = plan_dialog_rows(&props(&app, 0), 80);
            assert!(texts(&rows).iter().any(|row| row.starts_with("No plan p1 is waiting")));
            // The daemon says it expired: the dialog closes and says so.
            plan_settled(&mut app, &settled("p1", PlanDecision::Expired));
            assert_eq!(app.mode, Mode::Browse);
            assert_eq!(
                app.messages.status_message.as_deref(),
                Some("The plan expired unanswered: nothing was created")
            );
        });
    }

    #[test]
    fn plans_wait_their_turn_oldest_first_and_close_when_answered_elsewhere() {
        let (mut app, _rx) = app(AppProps::default());
        plan_pending(&mut app, pending("later", 5));
        plan_pending(&mut app, pending("first", 1));
        // The older one shows first; the newer waits after it.
        assert_eq!(shown(&app), Some("first"));
        assert_eq!(app.plans.queue.len(), 2);
        // Sent again (a reconnect): still one of each.
        plan_pending(&mut app, pending("later", 5));
        assert_eq!(app.plans.queue.len(), 2);
        // Answered in another Deckhand: the next one shows, from the top, with keys quiet again.
        app.dialog.scroll = 3;
        plan_settled(&mut app, &settled("first", PlanDecision::Approved));
        assert_eq!(shown(&app), Some("later"));
        assert_eq!(app.dialog.scroll, 0);
        assert!(app.plans.held_since.is_some());
        assert_eq!(
            app.messages.status_message.as_deref(),
            Some("The plan was approved in another Deckhand")
        );
        // A plan settled that is not shown just leaves the queue.
        plan_pending(&mut app, pending("third", 9));
        plan_settled(&mut app, &settled("third", PlanDecision::Expired));
        assert_eq!(app.plans.queue.len(), 1);
        plan_settled(&mut app, &settled("later", PlanDecision::Rejected));
        assert_eq!(app.mode, Mode::Browse);
        // Losing the daemon drops the queue: the next subscribe sends what still waits.
        plan_pending(&mut app, pending("again", 2));
        connection_lost(&mut app);
        assert_eq!(app.mode, Mode::Browse);
        assert!(app.plans.queue.is_empty());
    }

    #[test]
    fn a_plan_waits_behind_a_form_and_says_so_without_touching_what_was_typed() {
        let (mut app, _rx) = app(AppProps::default());
        app.set_mode(Mode::EnterName);
        app.create.draft_name = "half a na".to_owned();
        plan_pending(&mut app, pending("p1", 0));
        assert_eq!(app.mode, Mode::EnterName);
        assert_eq!(app.create.draft_name, "half a na");
        assert!(plan_waiting_hidden(&app));
        let messages: Vec<String> = footer_messages(&app).into_iter().map(|message| message.text).collect();
        assert!(messages.contains(&PLAN_WAITING_MESSAGE.to_owned()), "{messages:?}");
        assert!(row_text(&header_rows(&app)[0]).contains(PLAN_WAITING_HEADER));
        // Leaving the form for the session list opens it (the effect pass does this after each update).
        app.set_mode(Mode::Browse);
        open_if_free(&mut app);
        assert_eq!(shown(&app), Some("p1"));
        assert!(!plan_waiting_hidden(&app));
        assert!(!row_text(&header_rows(&app)[0]).contains(PLAN_WAITING_HEADER));
    }

    #[test]
    fn typing_to_an_agent_gives_way_to_the_plan_and_resumes_after_the_answer() {
        local(async {
            let (mut app, mut rx) = app(AppProps::default());
            let mut daemon = Daemon::connect(&mut app);
            app.data.repo_sessions = vec![session("a", "A", SessionStatus::Running)];
            app.set_selected_id(Some("a".to_owned()));
            browse::step_into_pane(&mut app);
            assert_eq!(app.mode, Mode::PreviewFocus);
            plan_pending(&mut app, pending("p1", 0));
            assert_eq!(shown(&app), Some("p1"));
            assert_eq!(app.preview_focus.session_id, None);
            press(&mut app, &esc());
            let decide = daemon.next().await;
            daemon.ok(&decide, json!({"ok": true})).await;
            apply(&mut app, &mut rx).await;
            assert_eq!(app.mode, Mode::PreviewFocus);
            assert_eq!(app.preview_focus.session_id.as_deref(), Some("a"));
            assert_eq!(
                app.messages.status_message.as_deref(),
                Some("Plan cancelled: nothing was created")
            );
        });
    }

    fn attach(session_id: &str) -> UiExitResult {
        UiExitResult::Attach {
            session_id: session_id.to_owned(),
            target: AttachTarget::Agent,
            title: Some("Orchestrator".to_owned()),
            cwd: Some("/repo".to_owned()),
            program: None,
            process: None,
            shell: None,
            shell_label: None,
            status_title: None,
        }
    }

    #[test]
    fn answering_the_plan_an_attach_was_left_for_attaches_again() {
        local(async {
            let props = AppProps {
                plan_reattach: Some(attach("orch")),
                ..AppProps::default()
            };
            let (mut app, mut rx) = app(props);
            let mut daemon = Daemon::connect(&mut app);
            app.data.repo_sessions = vec![session("orch", "Orchestrator", SessionStatus::Running)];
            plan_pending(&mut app, pending("p1", 0));
            press(&mut app, &enter());
            let decide = daemon.next().await;
            daemon.ok(&decide, json!({"ok": true})).await;
            apply(&mut app, &mut rx).await;
            assert_eq!(app.connection.exit, Some(attach("orch")));
        });
    }

    #[test]
    fn a_plan_answered_elsewhere_attaches_again_but_not_once_you_moved_on() {
        let props = || AppProps {
            plan_reattach: Some(attach("orch")),
            ..AppProps::default()
        };
        // Settled in another Deckhand (each attached terminal of the repository was detached for it): back too.
        let (mut elsewhere, _rx) = app(props());
        elsewhere.data.repo_sessions = vec![session("orch", "Orchestrator", SessionStatus::Running)];
        plan_pending(&mut elsewhere, pending("p1", 0));
        plan_settled(&mut elsewhere, &settled("p1", PlanDecision::Approved));
        assert_eq!(elsewhere.connection.exit, Some(attach("orch")));
        // Its session exited meanwhile: you stay on the session list.
        let (mut exited, _rx) = app(props());
        exited.data.repo_sessions = vec![session("orch", "Orchestrator", SessionStatus::Exited)];
        plan_pending(&mut exited, pending("p1", 0));
        plan_settled(&mut exited, &settled("p1", PlanDecision::Expired));
        assert_eq!(exited.connection.exit, None);
        assert_eq!(exited.plans.reattach, None);
        // A key on the session list before the plan came: you moved on.
        let (mut moved_on, _rx) = app(props());
        moved_on.handle_input(&text("j"));
        assert_eq!(moved_on.plans.reattach, None);
    }

    #[test]
    fn a_ui_that_came_back_for_a_plan_already_settled_forgets_the_attach() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .start_paused(true)
            .build()
            .unwrap_or_else(|error| panic!("{error}"));
        tokio::task::LocalSet::new().block_on(&runtime, async {
            let props = AppProps {
                plan_reattach: Some(attach("orch")),
                ..AppProps::default()
            };
            let (mut app, mut rx) = app(props);
            arm_reattach(&mut app);
            tokio::time::advance(REATTACH_WAIT).await;
            apply(&mut app, &mut rx).await;
            assert_eq!(app.plans.reattach, None);
            // With a plan waiting by then, it is kept.
            let props = AppProps {
                plan_reattach: Some(attach("orch")),
                ..AppProps::default()
            };
            let (mut app, mut rx) = crate::app::test_support::app(props);
            arm_reattach(&mut app);
            plan_pending(&mut app, pending("p1", 0));
            tokio::time::advance(REATTACH_WAIT).await;
            apply(&mut app, &mut rx).await;
            assert_eq!(app.plans.reattach, Some(attach("orch")));
        });
    }

    fn props(app: &App, now_ms: i64) -> PlanDialogProps<'_> {
        PlanDialogProps {
            plan: &app.plans.queue[0],
            more: app.plans.queue.len() - 1,
            answering: app.plans.answering,
            error: app.plans.error.as_deref(),
            now_ms,
        }
    }

    fn texts(rows: &DialogRows) -> Vec<String> {
        [&rows.header, &rows.body, &rows.footer]
            .into_iter()
            .flatten()
            .map(|row| row_text(row).trim_end().to_owned())
            .collect()
    }

    /// Noon and `minutes` (the plans above were created at 12:00 and later).
    fn at(minutes: f64) -> i64 {
        deckhand_core::jsstr::date::parse("2026-11-14T12:00:00.000Z").unwrap_or_default() + (minutes * 60_000.0) as i64
    }

    fn drawn(props: &PlanDialogProps<'_>, width: u16, height: u16, scroll: usize) -> Vec<String> {
        let area = Rect::new(0, 0, width, height);
        let mut buf = Buffer::empty(area);
        let view = DialogView {
            scroll,
            ..DialogView::default()
        };
        render_plan_dialog(props, &mut buf, area, view);
        (0..area.height)
            .map(|y| {
                let mut row = String::new();
                let mut x = 0;
                while x < area.width {
                    let symbol = buf[(x, y)].symbol();
                    row.push_str(symbol);
                    x += u16::try_from(string_width(symbol).max(1)).unwrap_or(1);
                }
                row.trim_end().to_owned()
            })
            .collect()
    }

    #[test]
    fn the_dialog_shows_the_tree_the_backlog_and_when_it_expires() {
        let (mut app, _rx) = app(AppProps::default());
        plan_pending(&mut app, pending("p1", 0));
        plan_pending(&mut app, pending("p2", 3));
        let lines = texts(&plan_dialog_rows(&props(&app, at(1.5)), 96));
        assert_eq!(lines[0], "Create this plan?");
        assert_eq!(
            lines[1],
            "Orchestrator's plan: 3 sessions, 1 task. Each session starts on its brief at once."
        );
        assert_eq!(
            lines[3..6],
            [
                "◇ Auth rework  (new worktree · claude)",
                "  ↳ Token storage  (stacked on Auth rework's branch · claude)",
                "◇ Migration script  (no worktree · codex)",
            ]
        );
        assert_eq!(
            lines[7..10],
            [
                "Backlog",
                "☐ Remove legacy endpoint",
                "  After Auth rework merges: the old routes go."
            ]
        );
        assert_eq!(lines[11], "Unanswered, it expires in 13 min. 1 more plan waiting.");
        assert_eq!(lines.last().map(String::as_str), Some("enter create • esc cancel"));
        assert_pane("63-confirm-plan", &drawn(&props(&app, at(1.5)), 98, 31, 0));
        // A plan run by hand has no caller; once answered the footer says what is being done.
        app.plans.queue[0].caller_title = None;
        app.plans.answering = Some(true);
        let lines = texts(&plan_dialog_rows(&props(&app, at(1.5)), 96));
        assert!(lines[1].starts_with("A plan of 3 sessions, 1 task."), "{lines:#?}");
        assert_eq!(lines.last().map(String::as_str), Some("Creating…"));
    }

    #[test]
    fn an_orchestrators_plan_says_its_idea_parks_its_tasks_under_it_and_shows_its_brief() {
        let (mut app, _rx) = app(AppProps::default());
        let mut plan = pending("p1", 0);
        plan.caller_title = Some("Rework auth".to_owned());
        plan.summary.idea = Some("Rework auth".to_owned());
        plan.summary.brief = Some(
            "Problem: tokens sit in plain text.\nApproach: an encrypted store.\nDecided: keep the API.\n\
             Ruled out: SSO.\nOpen: key rotation.\nOpen: the CLI.\n"
                .to_owned(),
        );
        plan_pending(&mut app, plan);
        let lines = texts(&plan_dialog_rows(&props(&app, at(1.5)), 96));
        assert_eq!(
            lines[1..3],
            [
                "A plan for ◎ Rework auth: 3 sessions, 1 task. Each session starts on its brief at once,",
                "under the orchestrator."
            ]
        );
        assert_eq!(lines[8], "Parked under the idea");
        let brief = lines
            .iter()
            .position(|line| line == "The idea's brief")
            .unwrap_or_default();
        assert_eq!(
            lines[brief + 1..brief + 6],
            [
                "Problem: tokens sit in plain text.",
                "Approach: an encrypted store.",
                "Decided: keep the API.",
                "Ruled out: SSO.",
                "… 2 more lines",
            ]
        );
        assert_pane("67-confirm-plan-idea", &drawn(&props(&app, at(1.5)), 98, 31, 0));
        // Tasks only: they are parked under the idea.
        app.plans.queue[0].summary.sessions.clear();
        let lines = texts(&plan_dialog_rows(&props(&app, at(1.5)), 96));
        assert!(lines[1].ends_with("Its tasks are parked under the idea."), "{lines:#?}");
    }

    #[test]
    fn a_long_plan_scrolls_and_keeps_its_answer_keys_and_error_in_sight() {
        let (mut app, _rx) = app(AppProps::default());
        let mut plan = pending("p1", 0);
        let entry = plan.summary.sessions[1].clone();
        for index in 0..20 {
            let mut child = entry.clone();
            child.key = format!("child{index}");
            child.title = format!("Child {index}");
            plan.summary.sessions.insert(1, child);
        }
        plan.summary.counts = "23 sessions, 1 task".to_owned();
        plan_pending(&mut app, plan);
        app.plans.error = Some("No plan p1 is waiting for an answer (it was answered or expired)".to_owned());
        let frame = drawn(&props(&app, at(1.5)), 70, 20, 0);
        assert!(
            frame.iter().any(|row| row.contains("↓") && row.contains("more · J")),
            "{frame:#?}"
        );
        assert!(
            frame.iter().any(|row| row.contains("enter create • esc cancel")),
            "{frame:#?}"
        );
        assert_pane("64-confirm-plan-scrolled", &frame);
    }

    fn assert_pane(name: &str, frame: &[String]) {
        deckhand_testkit::assert_snapshot(format!("fixtures/ui/panes/{name}.txt"), &frame.join("\n"));
    }
}
