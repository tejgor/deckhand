//! The plan confirmation (confirm-plan): an orchestrator's plan (`deckhand plan apply`) creates nothing until it is
//! answered here. The daemon pushes each waiting plan (`plan-pending`, again after every `subscribe`, and when one is
//! put aside) and says when one is settled (`plan-settled`: answered by any Deckhand, or dropped because its
//! orchestrator ended). `deckhand plan apply` does not wait: the orchestrator is told the answer in a message.
//!
//! - **One at a time.** A new plan opens the dialog by itself; `enter` creates the shown one, `x` twice cancels it
//!   (`plan-decide`, with a note for the orchestrator typed after the first `x`), and the next new one follows.
//! - **Put aside.** `esc` (`←`, `h`) puts the shown plan aside (`plan-park`): it keeps waiting, the header says so
//!   (`◎ 1 plan waiting · P`), and it no longer opens by itself or ends an attach. `P` on the session list opens the
//!   oldest plan waiting again.
//! - **Never over your typing.** It takes over the session list and typing to an agent or a shell (giving the typing
//!   back after the answer). On any other screen (a form, Tasks, Settings, another dialog) a new plan waits until you
//!   leave it, while the header and the footer say a plan is waiting. Keys typed on as it opens by itself are not
//!   answers: it takes them only after a pause ([`QUIET_MS`]).
//! - **Attached.** An attach ends when a new plan arrives (`deckhand_client::attach::AttachEnd::PlanPending`) and the
//!   UI comes back with the attach in [`PlanState::reattach`]: once the plan is answered or put aside (by you, another
//!   Deckhand, or because its orchestrator ended) it attaches again. A key for anything else, or no plan to ask about
//!   within [`REATTACH_WAIT`] (it was settled before the UI came back), leaves you in Deckhand with that session
//!   selected.

use std::time::Duration;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use deckhand_core::display::{HintPart, error_message, fit_hint, strip_terminal_controls};
use deckhand_core::jsstr::date::now_ms;
use deckhand_core::jsstr::{js_len, path};
use deckhand_core::key::Input;
use deckhand_core::plan::entry_line;
use deckhand_core::session_scope::checkout_in_scope;
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
        keys: &[
            KeySpec::Named(NamedKey::Escape),
            KeySpec::Named(NamedKey::Left),
            KeySpec::Char("h"),
        ],
        action: "put the plan aside",
        help: HelpRef::Hidden("the dialog shows its own hint line"),
    },
    Binding {
        keys: &[KeySpec::Char("x")],
        action: "cancel the plan (press twice; a note for the orchestrator may come between)",
        help: HelpRef::Hidden("the dialog shows its own hint line"),
    },
    crate::keymap::SCROLL_DIALOG,
];

/// Keys within this long of the dialog opening by itself, or of the last key it ignored, are not taken as answers: a
/// sentence being typed to the agent (an `enter` in it) must not create the plan.
pub const QUIET_MS: i64 = 750;

/// How long a UI that came back for a plan waits for it before it stops meaning to attach again.
pub const REATTACH_WAIT: Duration = Duration::from_secs(10);

/// The longest note for the orchestrator a cancel takes (UTF-16 units).
pub const MAX_NOTE: usize = 500;

/// The answer sent for the shown plan.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Answer {
    Create,
    Cancel,
    PutAside,
}

/// The plans waiting for an answer and the dialog's state.
#[derive(Debug, Default)]
pub struct PlanState {
    /// Every plan waiting, oldest first.
    pub queue: Vec<PendingPlan>,
    /// The plan the dialog shows.
    pub shown: Option<String>,
    /// The answer sent for the shown plan, until the daemon settles it (or answers the request).
    pub answering: Option<Answer>,
    /// After the first `x`: the note for the orchestrator typed so far (the cancel line shows).
    pub cancelling: Option<String>,
    /// Why the last answer failed.
    pub error: Option<String>,
    /// When the dialog opened by itself, or the last key it held back came (epoch milliseconds): see [`QUIET_MS`].
    pub held_since: Option<i64>,
    /// The typing the dialog took over (preview or terminal focus) in that session, given back after the answer.
    pub refocus: Option<(Mode, String)>,
    /// The attach a plan ended: attached again after the answer.
    pub reattach: Option<UiExitResult>,
    /// Forgets `reattach` when no plan came to ask about ([`REATTACH_WAIT`]).
    pub reattach_timer: Option<Timer>,
}

fn parked(plan: &PendingPlan) -> bool {
    plan.parked == Some(true)
}

/// Whether `plan` belongs to this Deckhand's checkout: its orchestrator runs here, so this sidebar lists it and the
/// sessions the plan creates ([`checkout_in_scope`], as for sessions). A plan of another checkout of the repository
/// never opens by itself here: the header only says where it waits, and `P` opens it when this checkout has none.
pub fn ours(app: &App, plan: &PendingPlan) -> bool {
    checkout_in_scope(&plan.repo_root, &app.props.repo_root)
}

/// The oldest new plan (not put aside) of this checkout: the one that opens by itself.
fn oldest_new(app: &App) -> Option<String> {
    app.plans
        .queue
        .iter()
        .find(|plan| !parked(plan) && ours(app, plan))
        .map(|plan| plan.id.clone())
}

/// How another checkout is named: `⎇ <branch>` as a session working there records it, else `⎇ <its directory>`.
pub fn checkout_name(app: &App, checkout: &str) -> String {
    let root = path::resolve(checkout);
    let branch = app.data.repo_sessions.iter().find_map(|session| {
        let worktree = session.worktree.as_ref()?;
        let at = worktree.path.as_deref().map(path::resolve)?;
        (at == root)
            .then_some(worktree.branch.as_deref()?)
            .filter(|branch| !branch.is_empty())
    });
    match branch {
        Some(branch) => format!("⎇ {branch}"),
        None => {
            let base = path::basename(&root);
            format!("⎇ {}", if base.is_empty() { root.as_str() } else { base.as_str() })
        }
    }
}

/// On mount, with an attach to go back to: forgotten unless a new plan is waiting by [`REATTACH_WAIT`].
pub fn arm_reattach(app: &mut App) {
    if app.plans.reattach.is_none() {
        return;
    }
    app.plans.reattach_timer = Some(app.dispatch().after(REATTACH_WAIT, |app: &mut App| {
        app.plans.reattach_timer = None;
        if oldest_new(app).is_none() && app.mode != Mode::ConfirmPlan {
            app.plans.reattach = None;
        }
    }));
}

/// `plan-pending`: queues the plan (oldest first; one already queued is replaced, as when it was put aside) and shows
/// a new one if it can.
pub fn plan_pending(app: &mut App, plan: PendingPlan) {
    let queue = &mut app.plans.queue;
    match queue.iter_mut().find(|queued| queued.id == plan.id) {
        Some(queued) => *queued = plan,
        None => queue.push(plan),
    }
    // ISO timestamps sort as text.
    queue.sort_by(|a, b| a.created_at.cmp(&b.created_at).then_with(|| a.id.cmp(&b.id)));
    // An older new plan came after the one shown (a reconnect sends them again): it shows first, answered by nobody
    // yet. One opened with `P` (put aside, or another checkout's) stays.
    if let Some((shown, _)) =
        shown_plan(app).filter(|(plan, _)| app.mode == Mode::ConfirmPlan && !parked(plan) && ours(app, plan))
        && app.plans.answering.is_none()
        && app.plans.cancelling.is_none()
    {
        let shown = shown.id.clone();
        if let Some(oldest) = oldest_new(app).filter(|oldest| *oldest != shown) {
            show(app, oldest, true);
        }
    }
    open_if_free(app);
}

/// `plan-settled`: the plan is no longer waiting (answered by any Deckhand, or its orchestrator ended). The dialog
/// moves on.
pub fn plan_settled(app: &mut App, settled: &PlanSettled) {
    if shown_id(app).as_deref() == Some(settled.plan_id.as_str()) {
        close_shown(app, Some(settled.decision));
        return;
    }
    app.plans.queue.retain(|plan| plan.id != settled.plan_id);
}

/// The connection to the daemon went: what was waiting is sent again on the next `subscribe`, and what was settled
/// meanwhile is not, so the queue starts over.
pub fn connection_lost(app: &mut App) {
    app.plans.queue.clear();
    app.plans.shown = None;
    app.plans.answering = None;
    app.plans.cancelling = None;
    app.plans.error = None;
    app.plans.refocus = None;
    if app.mode == Mode::ConfirmPlan {
        app.set_mode(Mode::Browse);
    }
}

/// The plan the dialog shows.
fn shown_id(app: &App) -> Option<String> {
    (app.mode == Mode::ConfirmPlan)
        .then(|| app.plans.shown.clone())
        .flatten()
}

/// The plan the dialog shows, and how many others wait.
pub fn shown_plan(app: &App) -> Option<(&PendingPlan, usize)> {
    let id = app.plans.shown.as_deref()?;
    let plan = app.plans.queue.iter().find(|plan| plan.id == id)?;
    Some((plan, app.plans.queue.len() - 1))
}

/// The plans waiting that the dialog does not show, as the header counts them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PlansWaiting {
    /// This checkout's (`P` opens the oldest).
    pub here: usize,
    /// Other checkouts' of the repository: how many, and the checkout's name when they all wait in one.
    pub elsewhere: usize,
    pub elsewhere_in: Option<String>,
}

/// How many plans wait that the dialog does not show (the header says so), this checkout's and other checkouts'.
pub fn plans_waiting_hidden(app: &App) -> PlansWaiting {
    if app.mode == Mode::ConfirmPlan {
        return PlansWaiting::default();
    }
    let (here, elsewhere): (Vec<&PendingPlan>, Vec<&PendingPlan>) =
        app.plans.queue.iter().partition(|plan| ours(app, plan));
    let first = elsewhere.first().map(|plan| path::resolve(&plan.repo_root));
    let one_checkout = elsewhere
        .iter()
        .all(|plan| Some(path::resolve(&plan.repo_root)) == first);
    PlansWaiting {
        here: here.len(),
        elsewhere: elsewhere.len(),
        elsewhere_in: first
            .filter(|_| one_checkout)
            .map(|checkout| checkout_name(app, &checkout)),
    }
}

/// Whether a new plan (not put aside) of this checkout waits behind the screen shown, to open once you close it (the
/// footer says so).
pub fn new_plan_waiting_hidden(app: &App) -> bool {
    app.mode != Mode::ConfirmPlan && oldest_new(app).is_some()
}

/// How many plans the session `id` applied wait for an answer (its details say so).
pub fn plans_of(app: &App, id: &str) -> usize {
    app.plans
        .queue
        .iter()
        .filter(|plan| plan.caller_session_id.as_deref() == Some(id))
        .count()
}

/// The header's word for `count` plans of this checkout waiting: `◎ 1 plan waiting · P`.
pub fn waiting_text(count: usize) -> String {
    let plans = if count == 1 { "plan" } else { "plans" };
    format!("◎ {count} {plans} waiting · P")
}

/// The header's quiet note for plans waiting in other checkouts: `◎ 1 plan waiting in ⎇ fix-auth`, or after this
/// checkout's (`more`) `1 more in ⎇ fix-auth`; `in other checkouts` when they wait in several.
pub fn elsewhere_text(waiting: &PlansWaiting) -> Option<String> {
    let count = waiting.elsewhere;
    if count == 0 {
        return None;
    }
    let place = waiting
        .elsewhere_in
        .as_deref()
        .map_or_else(|| "other checkouts".to_owned(), str::to_owned);
    if waiting.here > 0 {
        return Some(format!("{count} more in {place}"));
    }
    let plans = if count == 1 { "plan" } else { "plans" };
    Some(format!("◎ {count} {plans} waiting in {place}"))
}

/// Shows the oldest new plan (not put aside) of this checkout when nothing would be lost: from the session list, or
/// typing to an agent or a shell (left, and given back after the answer). Runs as an effect too, so a plan that waited
/// opens as you leave a screen. Another checkout's plan never opens by itself.
pub fn open_if_free(app: &mut App) {
    if app.mode == Mode::ConfirmPlan || app.messages.busy || app.connection.exit.is_some() {
        return;
    }
    let Some(id) = oldest_new(app) else {
        return;
    };
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
    show(app, id, true);
}

/// `P` on the session list: the oldest plan of this checkout waiting, put aside or not; only when there is none, the
/// oldest of another checkout, to answer it from here.
pub fn reopen(app: &mut App) {
    let oldest = |here: bool| {
        app.plans
            .queue
            .iter()
            .find(|plan| ours(app, plan) == here)
            .map(|plan| plan.id.clone())
    };
    let Some(id) = oldest(true).or_else(|| oldest(false)) else {
        app.set_status(Some("No plan is waiting".to_owned()));
        return;
    };
    app.set_mode(Mode::ConfirmPlan);
    show(app, id, false);
}

/// The dialog starts over for plan `id`: no answer, no error, at the top; keys quiet for a moment when it opened by
/// itself (`quiet`).
fn show(app: &mut App, id: String, quiet: bool) {
    app.plans.shown = Some(id);
    app.plans.answering = None;
    app.plans.cancelling = None;
    app.plans.error = None;
    app.plans.held_since = quiet.then(now_ms);
    app.dialog = DialogState::default();
}

/// The shown plan was settled (`decision`; `None`: put aside): the next new plan shows, or the dialog closes, saying
/// how it ended, and what the plans interrupted comes back.
fn close_shown(app: &mut App, decision: Option<PlanDecision>) {
    let Some(id) = app.plans.shown.take() else {
        return;
    };
    let answered_here = app.plans.answering.is_some();
    let plan = app.plans.queue.iter().find(|plan| plan.id == id).cloned();
    match decision {
        Some(_) => app.plans.queue.retain(|plan| plan.id != id),
        None => {
            // Another checkout's plan closes without being put aside: it still opens by itself in its own checkout.
            let here = plan.as_ref().is_some_and(|plan| ours(app, plan));
            if let Some(plan) = app.plans.queue.iter_mut().find(|plan| plan.id == id).filter(|_| here) {
                plan.parked = Some(true);
            }
        }
    }
    if let Some(plan) = plan {
        let text = if decision.is_none() && !ours(app, &plan) {
            format!("The plan still waits in {}", checkout_name(app, &plan.repo_root))
        } else {
            settled_text(&plan, decision, answered_here)
        };
        app.set_status(Some(text));
    }
    if let Some(next) = oldest_new(app) {
        show(app, next, true);
        return;
    }
    app.plans.answering = None;
    app.plans.cancelling = None;
    app.plans.error = None;
    app.set_mode(Mode::Browse);
    let refocus = app.plans.refocus.take();
    let reattach = app.plans.reattach.take();
    give_back(app, refocus, reattach);
}

/// What the status line says once a plan is settled (`None`: put aside).
fn settled_text(plan: &PendingPlan, decision: Option<PlanDecision>, ours: bool) -> String {
    let told = if plan.caller_session_id.is_some() {
        " (the orchestrator is told)"
    } else {
        ""
    };
    match (decision, ours) {
        (None, _) => "Plan put aside: P opens it again".to_owned(),
        (Some(PlanDecision::Approved), true) => format!("Plan approved: creating {}{told}", plan.summary.counts),
        (Some(PlanDecision::Rejected), true) => format!("Plan cancelled: nothing was created{told}"),
        (Some(PlanDecision::Approved), false) => "The plan was approved in another Deckhand".to_owned(),
        (Some(PlanDecision::Rejected), false) => "The plan was cancelled in another Deckhand".to_owned(),
        (Some(PlanDecision::Dropped), _) => "The plan was dropped: its orchestrator ended".to_owned(),
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

/// confirm-plan's keys: `enter` creates the plan, `esc` (`←`, `h`) puts it aside, `x` twice cancels it; J/K scroll (in
/// `App::handle_input`).
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
    if app.plans.cancelling.is_some() {
        handle_cancel_line(app, input);
        return;
    }
    let key = &input.key;
    if key.return_ {
        answer(app, Answer::Create, None);
    } else if key.escape || key.left_arrow || (!key.ctrl && input.input == "h") {
        put_aside(app);
    } else if !key.ctrl && input.input == "x" {
        app.plans.cancelling = Some(String::new());
        app.plans.error = None;
    }
}

/// The cancel line, after the first `x`: `x` again (before any note) or `enter` cancels the plan, with the note typed
/// for the orchestrator; `esc` goes back. A plan applied by hand has nobody to tell: it takes no note.
fn handle_cancel_line(app: &mut App, input: &Input) {
    let key = &input.key;
    let note = app.plans.cancelling.clone().unwrap_or_default();
    if key.escape {
        app.plans.cancelling = None;
        return;
    }
    if key.return_ || (note.is_empty() && !key.ctrl && input.input == "x") {
        answer(
            app,
            Answer::Cancel,
            Some(note.trim().to_owned()).filter(|note| !note.is_empty()),
        );
        return;
    }
    let takes_note = shown_plan(app).is_some_and(|(plan, _)| plan.caller_session_id.is_some());
    if !takes_note || key.ctrl || key.meta {
        return;
    }
    let Some(field) = app.plans.cancelling.as_mut() else {
        return;
    };
    if key.backspace || key.delete {
        field.pop();
        return;
    }
    let typed = strip_terminal_controls(&input.input.replace("\r\n", " ").replace(['\r', '\n', '\t'], " "));
    for c in typed.chars() {
        if js_len(field) + c.len_utf16() > MAX_NOTE {
            break;
        }
        field.push(c);
    }
}

/// Puts the shown plan aside (`plan-park`; one opened with `P` is aside already) and moves on. Another checkout's plan
/// just closes: it never opened by itself here, and it must go on doing so in its own checkout.
fn put_aside(app: &mut App) {
    let Some((plan, _)) = shown_plan(app) else {
        return;
    };
    if parked(plan) || !ours(app, plan) {
        close_shown(app, None);
        return;
    }
    let plan_id = plan.id.clone();
    let Some(client) = app.client() else {
        app.plans.error = Some("still connecting to daemon".to_owned());
        return;
    };
    app.plans.answering = Some(Answer::PutAside);
    app.plans.error = None;
    let reply = client.plan_park(&plan_id);
    drop(app.dispatch().spawn(reply, move |app: &mut App, result| {
        let shown = shown_id(app).as_deref() == Some(plan_id.as_str());
        match result {
            Ok(()) if shown => close_shown(app, None),
            Ok(()) => {}
            Err(error) if shown => {
                app.plans.answering = None;
                app.plans.error = Some(error_message(&error));
            }
            Err(error) => app.set_error(Some(error_message(&error))),
        }
    }));
}

/// Sends the answer for the shown plan (`note`: the user's word to the orchestrator, cancelling). The dialog moves on
/// when the daemon settles it (or answers this request); a refusal shows in the dialog.
fn answer(app: &mut App, answer: Answer, note: Option<String>) {
    let Some(plan_id) = app.plans.shown.clone() else {
        return;
    };
    let Some(client) = app.client() else {
        app.plans.error = Some("still connecting to daemon".to_owned());
        return;
    };
    let approve = answer == Answer::Create;
    app.plans.answering = Some(answer);
    app.plans.error = None;
    let reply = client.plan_decide(&plan_id, approve, note.as_deref());
    drop(app.dispatch().spawn(reply, move |app: &mut App, result| {
        let shown = shown_id(app).as_deref() == Some(plan_id.as_str());
        let decision = if approve {
            PlanDecision::Approved
        } else {
            PlanDecision::Rejected
        };
        match result {
            Ok(()) if shown => close_shown(app, Some(decision)),
            Ok(()) => app.plans.queue.retain(|plan| plan.id != plan_id),
            Err(error) if shown => {
                app.plans.answering = None;
                app.plans.cancelling = None;
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
    /// Another checkout's plan: that checkout's name (`⎇ fix-auth`), said beside what it is for.
    pub checkout: Option<&'a str>,
    /// Plans waiting besides this one.
    pub more: usize,
    pub answering: Option<Answer>,
    /// After the first `x`: the note typed so far.
    pub cancelling: Option<&'a str>,
    pub error: Option<&'a str>,
}

fn text(text: impl Into<String>, style: TextStyle) -> Row {
    vec![Seg::new(text, style)]
}

/// How many lines of the idea's brief the dialog shows.
const BRIEF_ROWS: usize = 4;

/// The dialog `width` wide: what the plan holds (and the idea it is for), its tree of sessions, its parked work (or
/// backlog tasks), the idea's brief and how many more plans wait; the answer keys (or the answer being sent, or the
/// cancel line) at the bottom.
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
    let place = props
        .checkout
        .map_or_else(String::new, |checkout| format!(" in {checkout}"));
    let from = match (idea, plan.caller_title.as_deref().filter(|title| !title.is_empty())) {
        (Some(idea), _) => format!("A plan for ◎ {idea}{place}: {}.", summary.counts),
        (None, Some(caller)) => format!("{caller}'s plan{place}: {}.", summary.counts),
        (None, None) => format!("A plan of {}{place}.", summary.counts),
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
    if props.more > 0 {
        let plans = if props.more == 1 { "plan" } else { "plans" };
        rows.footer.extend(fit_row(
            &text(format!("{} more {plans} waiting.", props.more), muted),
            content_width,
            Wrap::Wrap,
        ));
    }
    if let Some(error) = props.error.filter(|error| !error.is_empty()) {
        rows.footer.extend(fit_row(
            &text(error, TextStyle::fg(THEME.error)),
            content_width,
            Wrap::Wrap,
        ));
    }
    let takes_note = plan.caller_session_id.is_some();
    if let (Some(note), None) = (props.cancelling, props.answering) {
        let mut line = vec![Seg::new("Cancel this plan? ", TextStyle::fg(THEME.warn).bold(true))];
        if takes_note {
            line.push(Seg::new("A note for the orchestrator (optional): ", muted));
            line.push(Seg::new(format!("{note}▏"), TextStyle::fg(THEME.accent_soft)));
        }
        rows.footer.extend(fit_row(&line, content_width, Wrap::Wrap));
    }
    let hint = match props.answering {
        Some(Answer::Create) => "Creating…".to_owned(),
        Some(Answer::Cancel) => "Cancelling…".to_owned(),
        Some(Answer::PutAside) => "Putting it aside…".to_owned(),
        None => {
            let parts: Vec<HintPart> = match props.cancelling {
                Some(note) if !note.is_empty() => vec!["enter cancel it with this note".into(), "esc back".into()],
                Some(_) => vec!["x or enter cancel it".into(), "esc back".into()],
                None => vec!["enter create".into(), "esc put aside".into(), "x x cancel".into()],
            };
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
    use deckhand_protocol::data::{PlanDecision, SessionRecord};

    use super::*;
    use crate::app::layout::{PLAN_WAITING_MESSAGE, footer_messages, header_rows};
    use crate::app::modes::browse::tests::{Daemon, apply, local, named, text};
    use crate::app::state::AppProps;
    use crate::app::test_support::{app, session};
    use crate::primitives::{row_text, string_width};

    /// A plan of Auth rework (with Token storage under it) and a script, with one backlog task, from the orchestrator
    /// `orch`; created `minute` minutes past noon.
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
        }))
        .unwrap_or_else(|error| panic!("{error}"))
    }

    /// [`pending`], of an orchestrator running in another checkout of the repository: the worktree
    /// `/wt/fix_coaching_tone`, on the branch `fix-coaching-tone`.
    fn elsewhere(id: &str, minute: u32) -> PendingPlan {
        let mut plan = pending(id, minute);
        plan.repo_root = "/wt/fix_coaching_tone".to_owned();
        plan.caller_session_id = Some("far".to_owned());
        plan
    }

    /// The session that works in `/wt/fix_coaching_tone`, launched from this checkout (`/repo`): it names that
    /// checkout by its branch.
    fn fix_session() -> SessionRecord {
        let mut record = session("fix", "Fix coaching tone", SessionStatus::Running);
        record.worktree = Some(
            serde_json::from_value(json!({"mode": "managed", "path": "/wt/fix_coaching_tone",
                                          "branch": "fix-coaching-tone"}))
            .unwrap_or_else(|error| panic!("{error}")),
        );
        record
    }

    /// [`pending`], put aside.
    fn parked(id: &str, minute: u32) -> PendingPlan {
        let mut plan = pending(id, minute);
        plan.parked = Some(true);
        plan
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

    fn backspace() -> Input {
        named(Key {
            backspace: true,
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
            .then_some(app.plans.shown.as_deref())
            .flatten()
    }

    fn header(app: &App) -> String {
        row_text(&header_rows(app)[0])
    }

    #[test]
    fn a_new_plan_opens_the_dialog_and_enter_creates_it() {
        local(async {
            let (mut app, mut rx) = app(AppProps::default());
            let mut daemon = Daemon::connect(&mut app);
            plan_pending(&mut app, pending("p1", 0));
            assert_eq!(shown(&app), Some("p1"));
            // Keys typed on as it opens are not answers; nor is anything but its keys.
            let opened = app.plans.held_since.unwrap_or_default();
            handle_confirm_plan_at(&mut app, &enter(), opened + 100);
            handle_confirm_plan_at(&mut app, &enter(), opened + 100 + QUIET_MS - 1);
            press(&mut app, &text("q"));
            assert_eq!(app.plans.answering, None);
            press(&mut app, &enter());
            assert_eq!(app.plans.answering, Some(Answer::Create));
            let decide = daemon.next().await;
            assert_eq!(decide["type"], "plan-decide");
            assert_eq!(decide["planId"], "p1");
            assert_eq!(decide["approve"], true);
            assert!(decide.get("note").is_none());
            // While the answer is on its way, keys do nothing more.
            press(&mut app, &esc());
            daemon.ok(&decide, json!({"ok": true})).await;
            apply(&mut app, &mut rx).await;
            assert_eq!(app.mode, Mode::Browse);
            assert!(app.plans.queue.is_empty());
            assert_eq!(
                app.messages.status_message.as_deref(),
                Some("Plan approved: creating 3 sessions, 1 task (the orchestrator is told)")
            );
            // Its plan-settled, arriving after, changes nothing.
            plan_settled(&mut app, &settled("p1", PlanDecision::Approved));
            assert_eq!(app.mode, Mode::Browse);
        });
    }

    #[test]
    fn x_twice_cancels_and_a_note_typed_between_goes_to_the_orchestrator() {
        local(async {
            let (mut app, mut rx) = app(AppProps::default());
            let mut daemon = Daemon::connect(&mut app);
            plan_pending(&mut app, pending("p1", 0));
            plan_pending(&mut app, pending("p2", 1));
            // x opens the cancel line; esc goes back to the plan, which still waits.
            press(&mut app, &text("x"));
            assert_eq!(app.plans.cancelling.as_deref(), Some(""));
            assert!(app.typing_in_field());
            press(&mut app, &esc());
            assert_eq!((shown(&app), app.plans.cancelling.as_deref()), (Some("p1"), None));
            // x x: cancelled, no note.
            press(&mut app, &text("x"));
            press(&mut app, &text("x"));
            assert_eq!(app.plans.answering, Some(Answer::Cancel));
            let decide = daemon.next().await;
            assert_eq!(
                (decide["planId"].as_str(), decide["approve"].as_bool()),
                (Some("p1"), Some(false))
            );
            assert!(decide.get("note").is_none());
            daemon.ok(&decide, json!({"ok": true})).await;
            apply(&mut app, &mut rx).await;
            assert_eq!(
                app.messages.status_message.as_deref(),
                Some("Plan cancelled: nothing was created (the orchestrator is told)")
            );
            // The next one shows; x, then a note (x is a letter once there is text), then enter.
            assert_eq!(shown(&app), Some("p2"));
            press(&mut app, &text("x"));
            press(&mut app, &text("Split the tests out"));
            press(&mut app, &text("x"));
            press(&mut app, &backspace());
            press(&mut app, &text("\u{1b}[200~ please\r\nthanks\u{1b}[201~"));
            assert_eq!(
                app.plans.cancelling.as_deref(),
                Some("Split the tests out please thanks")
            );
            let rows = plan_dialog_rows(&props(&app), 96);
            let lines = texts(&rows);
            assert!(
                lines.contains(
                    &"Cancel this plan? A note for the orchestrator (optional): Split the tests out please thanks▏"
                        .to_owned()
                ),
                "{lines:#?}"
            );
            assert_eq!(
                lines.last().map(String::as_str),
                Some("enter cancel it with this note • esc back")
            );
            press(&mut app, &enter());
            let decide = daemon.next().await;
            assert_eq!(decide["planId"], "p2");
            assert_eq!(decide["approve"], false);
            assert_eq!(decide["note"], "Split the tests out please thanks");
        });
    }

    #[test]
    fn a_plan_applied_by_hand_cancels_without_a_note() {
        local(async {
            let (mut app, _rx) = app(AppProps::default());
            let mut daemon = Daemon::connect(&mut app);
            let mut plan = pending("p1", 0);
            plan.caller_session_id = None;
            plan.caller_title = None;
            plan_pending(&mut app, plan);
            press(&mut app, &text("x"));
            // Nobody to tell: letters are not taken as a note.
            press(&mut app, &text("why"));
            assert_eq!(app.plans.cancelling.as_deref(), Some(""));
            let lines = texts(&plan_dialog_rows(&props(&app), 96));
            assert!(lines.contains(&"Cancel this plan?".to_owned()), "{lines:#?}");
            assert_eq!(
                lines.last().map(String::as_str),
                Some("x or enter cancel it • esc back")
            );
            press(&mut app, &enter());
            let decide = daemon.next().await;
            assert_eq!(decide["approve"], false);
            assert!(decide.get("note").is_none());
        });
    }

    #[test]
    fn esc_puts_the_plan_aside_and_p_opens_it_again() {
        local(async {
            let (mut app, mut rx) = app(AppProps::default());
            let mut daemon = Daemon::connect(&mut app);
            app.data.repo_sessions = vec![session("orch", "Orchestrator", SessionStatus::Running)];
            plan_pending(&mut app, pending("p1", 0));
            assert!(!header(&app).contains("plan waiting"));
            press(&mut app, &esc());
            assert_eq!(app.plans.answering, Some(Answer::PutAside));
            let park = daemon.next().await;
            assert_eq!(park["type"], "plan-park");
            assert_eq!(park["planId"], "p1");
            daemon.ok(&park, json!({"ok": true})).await;
            apply(&mut app, &mut rx).await;
            // Closed, still waiting: the header says so and which key opens it; nothing was answered.
            assert_eq!(app.mode, Mode::Browse);
            assert_eq!(app.plans.queue.len(), 1);
            assert_eq!(
                app.messages.status_message.as_deref(),
                Some("Plan put aside: P opens it again")
            );
            assert!(header(&app).contains("◎ 1 plan waiting · P"), "{}", header(&app));
            // No footer line: it does not open by itself, not even when the daemon sends it again (parked).
            assert!(
                !footer_messages(&app)
                    .iter()
                    .any(|message| message.text == PLAN_WAITING_MESSAGE)
            );
            plan_pending(&mut app, parked("p1", 0));
            assert_eq!(app.mode, Mode::Browse);
            // P on the session list opens it, at once (no quiet moment); esc closes it again without asking anyone.
            app.handle_input(&text("P"));
            assert_eq!(shown(&app), Some("p1"));
            assert_eq!(app.plans.held_since, None);
            press(&mut app, &text("h"));
            assert_eq!(app.mode, Mode::Browse);
            // Opened again, enter creates it (the next request: putting it aside again sent nothing).
            app.handle_input(&text("P"));
            press(&mut app, &enter());
            let decide = daemon.next().await;
            assert_eq!(decide["type"], "plan-decide");
            assert_eq!(
                (decide["planId"].as_str(), decide["approve"].as_bool()),
                (Some("p1"), Some(true))
            );
            // With nothing waiting, P says so.
            let (mut empty, _rx) = crate::app::test_support::app(AppProps::default());
            empty.handle_input(&text("P"));
            assert_eq!(empty.mode, Mode::Browse);
            assert_eq!(empty.messages.status_message.as_deref(), Some("No plan is waiting"));
        });
    }

    #[test]
    fn a_refusal_shows_in_the_dialog_until_the_plan_is_dropped() {
        local(async {
            let (mut app, mut rx) = app(AppProps::default());
            let mut daemon = Daemon::connect(&mut app);
            plan_pending(&mut app, pending("p1", 0));
            press(&mut app, &enter());
            let decide = daemon.next().await;
            daemon
                .fail(
                    &decide,
                    "No plan p1 is waiting for an answer (it was answered already, or its orchestrator ended)",
                )
                .await;
            apply(&mut app, &mut rx).await;
            assert_eq!(shown(&app), Some("p1"));
            assert_eq!(app.plans.answering, None);
            let error = app.plans.error.clone().unwrap_or_default();
            assert!(error.starts_with("No plan p1 is waiting"), "{error}");
            let rows = plan_dialog_rows(&props(&app), 80);
            assert!(texts(&rows).iter().any(|row| row.starts_with("No plan p1 is waiting")));
            // The daemon says its orchestrator ended: the dialog closes and says so.
            plan_settled(&mut app, &settled("p1", PlanDecision::Dropped));
            assert_eq!(app.mode, Mode::Browse);
            assert_eq!(
                app.messages.status_message.as_deref(),
                Some("The plan was dropped: its orchestrator ended")
            );
        });
    }

    #[test]
    fn new_plans_open_oldest_first_and_plans_put_aside_wait_for_p() {
        let (mut app, _rx) = app(AppProps::default());
        // One put aside earlier (sent again after a subscribe) does not open by itself.
        plan_pending(&mut app, parked("aside", 0));
        assert_eq!(app.mode, Mode::Browse);
        plan_pending(&mut app, pending("later", 5));
        plan_pending(&mut app, pending("first", 1));
        // The oldest new one shows first; the dialog counts every other plan waiting.
        assert_eq!(shown(&app), Some("first"));
        assert_eq!(app.plans.queue.len(), 3);
        assert_eq!(props(&app).more, 2);
        // Sent again (a reconnect): still one of each, and the dialog stays on what it shows.
        plan_pending(&mut app, pending("later", 5));
        assert_eq!((app.plans.queue.len(), shown(&app)), (3, Some("first")));
        // Answered in another Deckhand: the next new one shows, from the top, with keys quiet again.
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
        plan_settled(&mut app, &settled("third", PlanDecision::Dropped));
        assert_eq!(app.plans.queue.len(), 2);
        // The last new one answered: the one put aside stays aside.
        plan_settled(&mut app, &settled("later", PlanDecision::Rejected));
        assert_eq!(app.mode, Mode::Browse);
        assert_eq!(app.plans.queue.len(), 1);
        // P shows the oldest plan waiting.
        reopen(&mut app);
        assert_eq!(shown(&app), Some("aside"));
        // Losing the daemon drops the queue: the next subscribe sends what still waits.
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
        assert!(new_plan_waiting_hidden(&app));
        let messages: Vec<String> = footer_messages(&app).into_iter().map(|message| message.text).collect();
        assert!(messages.contains(&PLAN_WAITING_MESSAGE.to_owned()), "{messages:?}");
        assert!(header(&app).contains("◎ 1 plan waiting · P"));
        // Leaving the form for the session list opens it (the effect pass does this after each update).
        app.set_mode(Mode::Browse);
        open_if_free(&mut app);
        assert_eq!(shown(&app), Some("p1"));
        assert!(!new_plan_waiting_hidden(&app));
        assert!(!header(&app).contains("plan waiting"));
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
            let park = daemon.next().await;
            daemon.ok(&park, json!({"ok": true})).await;
            apply(&mut app, &mut rx).await;
            assert_eq!(app.mode, Mode::PreviewFocus);
            assert_eq!(app.preview_focus.session_id.as_deref(), Some("a"));
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
    fn answering_or_putting_aside_the_plan_an_attach_was_left_for_attaches_again() {
        local(async {
            for key in [enter(), esc()] {
                let props = AppProps {
                    plan_reattach: Some(attach("orch")),
                    ..AppProps::default()
                };
                let (mut app, mut rx) = app(props);
                let mut daemon = Daemon::connect(&mut app);
                app.data.repo_sessions = vec![session("orch", "Orchestrator", SessionStatus::Running)];
                // A plan put aside earlier comes along; the new one is what the attach ended for.
                plan_pending(&mut app, parked("old", 0));
                plan_pending(&mut app, pending("p1", 1));
                assert_eq!(shown(&app), Some("p1"));
                press(&mut app, &key);
                let request = daemon.next().await;
                daemon.ok(&request, json!({"ok": true})).await;
                apply(&mut app, &mut rx).await;
                assert_eq!(app.connection.exit, Some(attach("orch")), "{}", request["type"]);
            }
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
        plan_settled(&mut exited, &settled("p1", PlanDecision::Dropped));
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
            let props = || AppProps {
                plan_reattach: Some(attach("orch")),
                ..AppProps::default()
            };
            // Only plans put aside wait: none to ask about.
            let (mut app, mut rx) = app(props());
            arm_reattach(&mut app);
            plan_pending(&mut app, parked("old", 0));
            tokio::time::advance(REATTACH_WAIT).await;
            apply(&mut app, &mut rx).await;
            assert_eq!(app.plans.reattach, None);
            // With a new plan waiting by then, it is kept.
            let (mut app, mut rx) = crate::app::test_support::app(props());
            arm_reattach(&mut app);
            plan_pending(&mut app, pending("p1", 0));
            tokio::time::advance(REATTACH_WAIT).await;
            apply(&mut app, &mut rx).await;
            assert_eq!(app.plans.reattach, Some(attach("orch")));
        });
    }

    #[test]
    fn another_checkouts_plan_waits_quietly_and_never_opens_by_itself() {
        local(async {
            let (mut app, _rx) = app(AppProps::default());
            let _daemon = Daemon::connect(&mut app);
            app.data.repo_sessions = vec![session("a", "A", SessionStatus::Running), fix_session()];
            // Typing to an agent: the plan does not take it over.
            app.set_selected_id(Some("a".to_owned()));
            browse::step_into_pane(&mut app);
            plan_pending(&mut app, elsewhere("far1", 0));
            assert_eq!(app.mode, Mode::PreviewFocus);
            assert_eq!(app.preview_focus.session_id.as_deref(), Some("a"));
            // On the session list, still not; no footer line, only the header's note naming the checkout, no `P`.
            app.set_mode(Mode::Browse);
            open_if_free(&mut app);
            assert_eq!(app.mode, Mode::Browse);
            assert!(!new_plan_waiting_hidden(&app));
            assert!(
                !footer_messages(&app)
                    .iter()
                    .any(|message| message.text == PLAN_WAITING_MESSAGE)
            );
            assert!(
                header(&app).contains("◎ 1 plan waiting in ⎇ fix-coaching-tone   "),
                "{}",
                header(&app)
            );
            assert!(!header(&app).contains("· P"), "{}", header(&app));
            // A narrow header drops the note first.
            app.browse.terminal_size.cols = 40;
            assert!(!header(&app).contains("plan waiting"), "{}", header(&app));
            assert!(header(&app).contains("● "), "{}", header(&app));
            app.browse.terminal_size.cols = 130;
            // A second one there, of a checkout no session names: by its directory; two checkouts: not named.
            plan_pending(&mut app, elsewhere("far2", 1));
            assert!(
                header(&app).contains("◎ 2 plans waiting in ⎇ fix-coaching-tone"),
                "{}",
                header(&app)
            );
            let mut third = elsewhere("far3", 2);
            third.repo_root = "/wt/other_idea".to_owned();
            assert_eq!(checkout_name(&app, &third.repo_root), "⎇ other_idea");
            plan_pending(&mut app, third);
            assert!(
                header(&app).contains("◎ 3 plans waiting in other checkouts"),
                "{}",
                header(&app)
            );
        });
    }

    #[test]
    fn another_checkouts_plan_does_not_reattach_or_count_for_an_attach_it_did_not_end() {
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
            plan_pending(&mut app, elsewhere("far1", 0));
            assert_eq!(app.mode, Mode::Browse);
            tokio::time::advance(REATTACH_WAIT).await;
            apply(&mut app, &mut rx).await;
            assert_eq!(app.plans.reattach, None);
            assert_eq!(app.connection.exit, None);
        });
    }

    #[test]
    fn p_opens_this_checkouts_plan_first_and_another_checkouts_only_when_there_is_none() {
        local(async {
            let (mut app, mut rx) = app(AppProps::default());
            let mut daemon = Daemon::connect(&mut app);
            app.data.repo_sessions = vec![session("orch", "Orchestrator", SessionStatus::Running), fix_session()];
            // The other checkout's is older; this checkout's opens by itself all the same, and is put aside.
            plan_pending(&mut app, elsewhere("far", 0));
            plan_pending(&mut app, pending("p1", 5));
            assert_eq!(shown(&app), Some("p1"));
            press(&mut app, &esc());
            let park = daemon.next().await;
            assert_eq!(park["planId"], "p1");
            daemon.ok(&park, json!({"ok": true})).await;
            apply(&mut app, &mut rx).await;
            // Both waiting: this checkout's first, with its key; the other's after it, quiet.
            let both = header(&app);
            assert!(
                both.contains("◎ 1 plan waiting · P   1 more in ⎇ fix-coaching-tone   "),
                "{both}"
            );
            app.handle_input(&text("P"));
            assert_eq!(shown(&app), Some("p1"));
            press(&mut app, &enter());
            let decide = daemon.next().await;
            assert_eq!(decide["planId"], "p1");
            daemon.ok(&decide, json!({"ok": true})).await;
            apply(&mut app, &mut rx).await;
            // The other checkout's did not open next.
            assert_eq!(app.mode, Mode::Browse);
            // Now P opens it, deliberately; esc closes it without putting it aside (it still opens by itself in its own
            // checkout).
            app.handle_input(&text("P"));
            assert_eq!(shown(&app), Some("far"));
            press(&mut app, &esc());
            assert_eq!(app.mode, Mode::Browse);
            assert_eq!(app.plans.queue[0].parked, None);
            assert_eq!(
                app.messages.status_message.as_deref(),
                Some("The plan still waits in ⎇ fix-coaching-tone")
            );
            // Answered from here, as any plan: the daemon creates its sessions where it belongs.
            app.handle_input(&text("P"));
            press(&mut app, &enter());
            let decide = daemon.next().await;
            assert_eq!(
                (
                    decide["type"].as_str(),
                    decide["planId"].as_str(),
                    decide["approve"].as_bool()
                ),
                (Some("plan-decide"), Some("far"), Some(true))
            );
            daemon.ok(&decide, json!({"ok": true})).await;
            apply(&mut app, &mut rx).await;
            assert_eq!(app.mode, Mode::Browse);
            assert!(app.plans.queue.is_empty());
        });
    }

    #[test]
    fn a_new_plan_of_this_checkout_waits_behind_another_checkouts_plan_opened_with_p() {
        let (mut app, _rx) = app(AppProps::default());
        plan_pending(&mut app, elsewhere("far", 0));
        app.handle_input(&text("P"));
        assert_eq!(shown(&app), Some("far"));
        plan_pending(&mut app, pending("p1", 1));
        assert_eq!(shown(&app), Some("far"));
        // Closing it shows this checkout's.
        press(&mut app, &esc());
        assert_eq!(shown(&app), Some("p1"));
    }

    #[test]
    fn the_dialog_names_the_checkout_of_another_checkouts_plan() {
        let (mut app, _rx) = app(AppProps::default());
        app.data.repo_sessions = vec![fix_session()];
        let mut plan = elsewhere("far", 0);
        plan.summary.idea = Some("Coaching tone".to_owned());
        plan_pending(&mut app, plan);
        app.handle_input(&text("P"));
        let (plan, more) = shown_plan(&app).unwrap_or_else(|| panic!("no plan shown"));
        let checkout = (!ours(&app, plan)).then(|| checkout_name(&app, &plan.repo_root));
        assert_eq!(checkout.as_deref(), Some("⎇ fix-coaching-tone"));
        let props = PlanDialogProps {
            checkout: checkout.as_deref(),
            ..props(&app)
        };
        assert_eq!(more, 0);
        let lines = texts(&plan_dialog_rows(&props, 120));
        assert!(
            lines[1].starts_with("A plan for ◎ Coaching tone in ⎇ fix-coaching-tone: 3 sessions, 1 task."),
            "{lines:?}"
        );
        // This checkout's names none.
        assert!(ours(&app, &pending("p1", 0)));
    }

    fn props(app: &App) -> PlanDialogProps<'_> {
        let (plan, more) = shown_plan(app).unwrap_or_else(|| panic!("no plan shown"));
        PlanDialogProps {
            plan,
            checkout: None,
            more,
            answering: app.plans.answering,
            cancelling: app.plans.cancelling.as_deref(),
            error: app.plans.error.as_deref(),
        }
    }

    fn texts(rows: &DialogRows) -> Vec<String> {
        [&rows.header, &rows.body, &rows.footer]
            .into_iter()
            .flatten()
            .map(|row| row_text(row).trim_end().to_owned())
            .collect()
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
    fn the_dialog_shows_the_tree_the_backlog_and_how_many_more_wait() {
        let (mut app, _rx) = app(AppProps::default());
        plan_pending(&mut app, pending("p1", 0));
        plan_pending(&mut app, pending("p2", 3));
        let lines = texts(&plan_dialog_rows(&props(&app), 96));
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
        assert_eq!(lines[11], "1 more plan waiting.");
        assert_eq!(
            lines.last().map(String::as_str),
            Some("enter create • esc put aside • x x cancel")
        );
        assert_pane("63-confirm-plan", &drawn(&props(&app), 98, 31, 0));
        // A plan run by hand has no caller; once answered the footer says what is being done.
        app.plans.queue[0].caller_title = None;
        app.plans.answering = Some(Answer::Create);
        let lines = texts(&plan_dialog_rows(&props(&app), 96));
        assert!(lines[1].starts_with("A plan of 3 sessions, 1 task."), "{lines:#?}");
        assert_eq!(lines.last().map(String::as_str), Some("Creating…"));
        app.plans.answering = Some(Answer::PutAside);
        let lines = texts(&plan_dialog_rows(&props(&app), 96));
        assert_eq!(lines.last().map(String::as_str), Some("Putting it aside…"));
    }

    #[test]
    fn the_cancel_line_asks_for_a_note_for_the_orchestrator() {
        let (mut app, _rx) = app(AppProps::default());
        plan_pending(&mut app, pending("p1", 0));
        app.plans.held_since = None;
        press(&mut app, &text("x"));
        let lines = texts(&plan_dialog_rows(&props(&app), 96));
        assert_eq!(
            lines[lines.len() - 2..],
            [
                "Cancel this plan? A note for the orchestrator (optional): ▏",
                "x or enter cancel it • esc back"
            ]
        );
        press(&mut app, &text("Too many sessions"));
        assert_pane("70-confirm-plan-cancel", &drawn(&props(&app), 98, 31, 0));
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
        let lines = texts(&plan_dialog_rows(&props(&app), 96));
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
        assert_pane("67-confirm-plan-idea", &drawn(&props(&app), 98, 31, 0));
        // Tasks only: they are parked under the idea.
        app.plans.queue[0].summary.sessions.clear();
        let lines = texts(&plan_dialog_rows(&props(&app), 96));
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
        app.plans.error =
            Some("No plan p1 is waiting for an answer (it was answered already, or its orchestrator ended)".to_owned());
        let frame = drawn(&props(&app), 70, 20, 0);
        assert!(
            frame.iter().any(|row| row.contains("↓") && row.contains("more · J")),
            "{frame:#?}"
        );
        assert!(
            frame
                .iter()
                .any(|row| row.contains("enter create • esc put aside • x x cancel")),
            "{frame:#?}"
        );
        assert_pane("64-confirm-plan-scrolled", &frame);
    }

    fn assert_pane(name: &str, frame: &[String]) {
        deckhand_testkit::assert_snapshot(format!("fixtures/ui/panes/{name}.txt"), &frame.join("\n"));
    }
}
