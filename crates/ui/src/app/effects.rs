//! Effects: every `useEffect` of App, run after each batch of events (React's effect phase), and the daemon
//! subscription (`createLiveClient` with its handlers, reconnecting 500 ms after a close).
//!
//! Each effect keeps its last dependencies here; when they change, the previous run is cleaned up (its request
//! aborted, its timer dropped, as TS's `cancelled` flags and cleanups do) and it runs again. Watch requests resolve
//! through the app's [`Dispatch`], so a late reply for another selection is dropped as in TS.
//!
//! React re-renders after an effect sets state and runs the effects whose dependencies changed; [`run_effects`] does
//! the same by repeating the pass until no effect runs (bounded). Within a pass the derived values (`sessions`,
//! `visibleSessions`, `selectedSession`) are those of the pass's start, as in one React commit.

use std::cell::Cell;
use std::future::Future;
use std::io::Write;
use std::rc::Rc;
use std::time::Duration;

use deckhand_client::attach::sanitize_terminal_title;
use deckhand_client::client::read_daemon_pid;
use deckhand_client::live::{LiveClientHandlers, create_live_client};
use deckhand_core::daemon_build::{notice_key, update_notice};
use deckhand_core::error::DhError;
use deckhand_core::jsstr::date;
use deckhand_core::sidebar_model::{ms_until_age_changes, status_since};
use deckhand_core::workspace::workspace_key;
use deckhand_protocol::BUILD_VERSION;
use deckhand_protocol::data::PingResult;
use deckhand_protocol::data::{
    AgentActivityStatus, AttentionState, DevRecord, RightPaneTab, SessionRecord, SessionStatus,
};
use deckhand_protocol::records::{SessionFilter, SessionView};
use deckhand_sys::async_util::{Task, spawn};
use deckhand_sys::storage::load_app_config;

use crate::app::event_loop::Dispatch;
use crate::app::layout::{measure_sidebar, normalize_sidebar_percent};
use crate::app::state::{App, AppTimers, Mode, empty_pane};
use crate::app::{
    Derived, ERROR_MESSAGE_AUTO_HIDE_MS, SPINNER_FRAMES, STATUS_MESSAGE_AUTO_HIDE_MS, normalize_scroll_sensitivity,
};
use crate::panes::dev::{dev_cell_size, dev_processes};
use crate::panes::terminal::TerminalView;
use deckhand_protocol::data::PreviewRecord;

/// One effect's last dependencies and the work it started.
#[derive(Debug)]
pub struct Watch<D> {
    pub deps: Option<D>,
    pub task: Option<Task<()>>,
    /// Bumped on every run: a reply for an older run is dropped (TS: that run's `cancelled` flag).
    pub generation: u64,
}

impl<D> Default for Watch<D> {
    fn default() -> Self {
        Watch {
            deps: None,
            task: None,
            generation: 0,
        }
    }
}

impl<D: PartialEq> Watch<D> {
    /// Starts a new run when `deps` changed: the previous one is cancelled; returns the new run's generation.
    fn restart(&mut self, deps: D) -> Option<u64> {
        if self.deps.as_ref() == Some(&deps) {
            return None;
        }
        self.cancel();
        self.deps = Some(deps);
        Some(self.generation)
    }

    /// Cancels the current run (its request aborted, a reply already queued ignored).
    fn cancel(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
        self.generation += 1;
    }
}

/// The dependencies of a pane watch (watch-preview, -terminal, -action, -changes, -dev): which client, session and
/// size it asked for, and the workspace/lifecycle scope (TS `selectedPaneScope`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PaneWatchDeps {
    /// TS `client` identity: the connection epoch it was created in, plus one (0: no client).
    pub epoch: u64,
    pub session_id: Option<String>,
    pub cols: u16,
    pub rows: u16,
    /// Preview only.
    pub scroll_offset: i64,
    pub pane_scope: Option<String>,
    /// The watching tab is shown (terminal, git, dev).
    pub active: bool,
    /// Preview only (not in the TS): you type to the agent, so its previews carry the styled screen.
    pub interactive: bool,
    /// Terminal only (not in the TS): the shell you type into, so its records carry its styled screen.
    pub typing_shell: Option<i64>,
}

/// TS: the dependencies of the `[selectedSession]` effect (the pane records reset for a new selection).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SelectedSessionDeps {
    pub session: Option<SessionRecord>,
}

/// Every effect's state.
#[derive(Debug, Default)]
pub struct EffectState {
    /// The mount effects ran (config load for the wheel speed, the first connection).
    pub mounted: bool,
    /// Effects run so far (a pass that runs none ends the effect phase).
    pub runs: u64,
    /// Terminal mode sequences (bracketed paste, mouse reporting) go to stdout; off in tests.
    pub stdout_modes: bool,
    /// Bracketed paste / mouse reporting are on (undone when the mode ends or the app unmounts).
    pub paste_on: bool,
    pub mouse_on: bool,
    /// Not in the TS: the terminal title last written (the terminal's own was saved before the first).
    pub title: Option<String>,
    pub selected_id: Option<Option<String>>,
    pub active_tab: Option<RightPaneTab>,
    pub mode: Option<Mode>,
    pub terminal_view: Option<TerminalView>,
    pub visibility: Option<(SessionFilter, String)>,
    pub session_view: Option<SessionView>,
    pub collapsed: Option<Vec<String>>,
    pub hidden_exited: Option<Vec<String>>,
    pub collapsed_task_steps: Option<Vec<String>>,
    pub selected_session: Option<SelectedSessionDeps>,
    /// TS `shouldAnimateStatus` at the last run (the spinner interval).
    pub animating: Option<bool>,
    /// The sessions, visible list and load state the selection was last reconciled with.
    pub selection_sync: Option<(Vec<SessionRecord>, Vec<String>, bool)>,
    pub status_message: Option<Option<String>>,
    pub error: Option<Option<String>>,
    pub numeric_selection: Option<String>,
    /// The selected session's status-since and done-at the age timer was set for.
    pub clock: Option<(Option<String>, Option<String>, u64, bool)>,
    /// The filtered existing-worktree count the picker's index was clamped to.
    pub worktree_count: Option<usize>,
    /// The task list text `tasks` was parsed from (TS: the `[tasksDoc?.text]` memo).
    pub tasks_text: Option<String>,
    pub preview: Watch<PaneWatchDeps>,
    pub terminal: Watch<PaneWatchDeps>,
    pub action: Watch<PaneWatchDeps>,
    pub changes: Watch<PaneWatchDeps>,
    pub dev: Watch<PaneWatchDeps>,
    /// The new worktree's base branches (enter-name with a new worktree): client key and the form's cwd.
    pub branches: Watch<(u64, String)>,
    /// The connection epoch the subscription runs for.
    pub connection: Option<u64>,
    /// The subscription run's `cancelled` flag.
    pub connection_cancel: Option<Rc<Cell<bool>>>,
}

/// The most passes one effect phase makes (an effect that keeps changing its own dependencies stops here).
const MAX_EFFECT_PASSES: usize = 16;

/// TS: scheduleReconnect's delay.
const RECONNECT_DELAY: Duration = Duration::from_millis(500);

/// TS: the spinner interval.
const SPINNER_PERIOD: Duration = Duration::from_millis(120);

/// TS: the numeric selection's confirmation delay.
const NUMERIC_SELECTION_DELAY: Duration = Duration::from_millis(300);

/// TS: the mouse modes preview focus turns on (and off, in reverse).
const MOUSE_ON: &str = "\u{1b}[?1000h\u{1b}[?1002h\u{1b}[?1003h\u{1b}[?1006h\u{1b}[?1015h\u{1b}[?1016h";
const MOUSE_OFF: &str = "\u{1b}[?1016l\u{1b}[?1015l\u{1b}[?1006l\u{1b}[?1003l\u{1b}[?1002l\u{1b}[?1000l";
const PASTE_ON: &str = "\u{1b}[?2004h";
const PASTE_OFF: &str = "\u{1b}[?2004l";

/// TS `shouldAnimateStatus`: a session starting, or running and working.
pub fn should_animate_status(sessions: &[SessionRecord]) -> bool {
    sessions.iter().any(|session| session_animates(session.status, session))
}

/// TS: whether `session` counts as working for the spinner (`starting`, or running with attention `working`, or an
/// active agent with unknown attention).
pub fn session_animates(status: SessionStatus, session: &SessionRecord) -> bool {
    let attention = session.attention.as_ref().map(|attention| attention.state);
    status == SessionStatus::Starting
        || (status == SessionStatus::Running
            && (attention == Some(AttentionState::Working)
                || (matches!(attention, None | Some(AttentionState::Unknown))
                    && session.agent_status == Some(AgentActivityStatus::Active))))
}

/// Not in the TS: whether any session the daemon sent works, not only this checkout's: the Tasks and Worktrees views
/// show every checkout's sessions, and a spinner drawn there for another checkout's session never moved.
fn animates(app: &App) -> bool {
    should_animate_status(&app.data.repo_sessions)
}

/// Counts an effect run.
fn ran(app: &mut App) {
    app.effects.runs += 1;
}

/// The key of the client an effect used (TS: the `client` dependency's identity).
fn client_key(app: &App) -> u64 {
    if app.connection.client.is_some() {
        app.connection.epoch + 1
    } else {
        0
    }
}

/// Runs every effect whose dependencies changed (TS: React's effect phase after a render).
pub fn run_effects(app: &mut App) {
    settle(app);
    // The flows' own effects (their state follows the app's), then whatever they changed.
    sync_flows(app);
    settle(app);
}

/// Effect passes until one runs no effect.
fn settle(app: &mut App) {
    for _ in 0..MAX_EFFECT_PASSES {
        let before = app.effects.runs;
        effect_pass(app);
        if app.effects.runs == before {
            break;
        }
    }
}

/// Not in the TS: measures the listed rows once they load (at launch) and after a resize, never while sessions come
/// and go (the sidebar would jump): it shrinks to the widest when every row fits in less than its share.
fn sidebar_fit_effect(app: &mut App, derived: &Derived) {
    if !app.browse.sidebar_fit_pending || !app.data.sessions_loaded {
        return;
    }
    app.browse.sidebar_fit_pending = false;
    measure_sidebar(app, derived);
}

/// One React commit's effects, in app.tsx's declaration order.
fn effect_pass(app: &mut App) {
    sync_tasks_memo(app);
    let derived = app.derived();
    terminal_view_effect(app);
    visibility_effect(app);
    session_view_effect(app);
    status_message_effect(app);
    error_effect(app);
    selected_id_effect(app);
    collapsed_effect(app);
    mount_effect(app);
    active_tab_effect(app);
    crate::app::modes::preview_focus::interactive_focus_effect(app);
    if app.effects.stdout_modes {
        let mut stdout = std::io::stdout();
        sync_terminal_modes(app, &mut stdout);
        if std::io::IsTerminal::is_terminal(&stdout) {
            sync_terminal_title(app, &mut stdout);
        }
    } else {
        sync_terminal_modes(app, &mut std::io::sink());
        sync_terminal_title(app, &mut std::io::sink());
    }
    spinner_effect(app);
    sidebar_fit_effect(app, &derived);
    connection_effect(app);
    selection_effect(app, &derived);
    numeric_selection_effect(app);
    clock_effect(app, &derived);
    worktree_index_effect(app);
    selected_session_effect(app, &derived);
    watch_effects(app, &derived);
    branches_effect(app);
}

/// TS: `tasks = useMemo(() => parseTasks(tasksDoc?.text ?? ''), [tasksDoc?.text])`.
fn sync_tasks_memo(app: &mut App) {
    let text = app
        .data
        .tasks_doc
        .as_ref()
        .map(|doc| doc.text.clone())
        .unwrap_or_default();
    if app.effects.tasks_text.as_ref() == Some(&text) {
        return;
    }
    app.data.tasks = deckhand_core::tasks::parse_tasks(&text);
    app.effects.tasks_text = Some(text);
}

/// TS: the flows' effects: the Changes flow's (while the Git tab or changes focus shows it) and the Notes flow's
/// `[focused]` effect (when focus starts or ends).
fn sync_flows(app: &mut App) {
    if app.browse.active_tab == RightPaneTab::Git || app.mode == Mode::ChangesFocus {
        app.changes_flow_call(|flow, props, cx| flow.sync(props, cx));
    }
    if app.mode == Mode::NotesFocus || app.flows.notes.was_focused {
        app.notes_flow_call(|flow, props, cx| flow.sync(props, cx));
    }
}

/// TS: `useEffect(() => onTerminalViewChange?.(terminalView), [terminalView])`.
fn terminal_view_effect(app: &mut App) {
    let view = app.panes.terminal_view;
    if app.effects.terminal_view == Some(view) {
        return;
    }
    app.effects.terminal_view = Some(view);
    ran(app);
    if let Some(listener) = &app.props.callbacks.on_terminal_view_change {
        listener(view);
    }
}

/// TS: `useEffect(() => onSessionVisibilityChange?.(sessionFilter, sessionQuery), [sessionFilter, sessionQuery])`.
fn visibility_effect(app: &mut App) {
    let deps = (app.browse.session_filter, app.browse.session_query.clone());
    if app.effects.visibility.as_ref() == Some(&deps) {
        return;
    }
    app.effects.visibility = Some(deps);
    ran(app);
    if let Some(listener) = &app.props.callbacks.on_session_visibility_change {
        listener(app.browse.session_filter, &app.browse.session_query);
    }
}

/// Not in the TS: reports the sidebar's view (`V`) to be saved.
fn session_view_effect(app: &mut App) {
    let view = app.browse.session_view;
    if app.effects.session_view == Some(view) {
        return;
    }
    app.effects.session_view = Some(view);
    ran(app);
    if let Some(listener) = &app.props.callbacks.on_session_view_change {
        listener(view);
    }
}

/// TS: the status message hides after STATUS_MESSAGE_AUTO_HIDE_MS (unless replaced meanwhile).
fn status_message_effect(app: &mut App) {
    let message = app.messages.status_message.clone();
    if app.effects.status_message.as_ref() == Some(&message) {
        return;
    }
    app.effects.status_message = Some(message.clone());
    ran(app);
    app.messages.status_timer = message.filter(|message| !message.is_empty()).map(|message| {
        app.connection.dispatch.after(
            Duration::from_millis(STATUS_MESSAGE_AUTO_HIDE_MS),
            move |app: &mut App| {
                hide_status(app, &message);
            },
        )
    });
}

/// The status timer's callback: hides `message` unless another replaced it.
fn hide_status(app: &mut App, message: &str) {
    if app.messages.status_message.as_deref() == Some(message) {
        app.messages.status_message = None;
    }
}

/// The error timer's callback: hides `error` unless another replaced it.
fn hide_error(app: &mut App, error: &str) {
    if app.messages.error.as_deref() == Some(error) {
        app.messages.error = None;
    }
}

/// TS: the error hides after ERROR_MESSAGE_AUTO_HIDE_MS (unless replaced meanwhile).
fn error_effect(app: &mut App) {
    let error = app.messages.error.clone();
    if app.effects.error.as_ref() == Some(&error) {
        return;
    }
    app.effects.error = Some(error.clone());
    ran(app);
    app.messages.error_timer = error.filter(|error| !error.is_empty()).map(|error| {
        app.connection.dispatch.after(
            Duration::from_millis(ERROR_MESSAGE_AUTO_HIDE_MS),
            move |app: &mut App| {
                hide_error(app, &error);
            },
        )
    });
}

/// TS: the `[selectedId]` effect: report it, reset the preview's scroll and show the session's last tab.
fn selected_id_effect(app: &mut App) {
    let id = app.browse.selected_id.clone();
    if app.effects.selected_id.as_ref() == Some(&id) {
        return;
    }
    app.effects.selected_id = Some(id.clone());
    ran(app);
    if let Some(listener) = &app.props.callbacks.on_selected_id_change {
        listener(id.as_deref());
    }
    app.preview_focus.scroll_offset = 0;
    if let Some(id) = id.filter(|id| !id.is_empty()) {
        app.browse.active_tab = app
            .browse
            .session_tabs
            .get(&id)
            .copied()
            .unwrap_or(RightPaneTab::Preview);
    }
}

/// TS: the `[collapsedSessionIds]` and `[hiddenExitedSessionIds]` effects (reported to cli.ts), and the collapsed task
/// steps.
fn collapsed_effect(app: &mut App) {
    let collapsed: Vec<String> = app.browse.collapsed_session_ids.iter().cloned().collect();
    if app.effects.collapsed.as_ref() != Some(&collapsed) {
        ran(app);
        if let Some(listener) = &app.props.callbacks.on_collapsed_session_ids_change {
            listener(&collapsed);
        }
        app.effects.collapsed = Some(collapsed);
    }
    let hidden: Vec<String> = app.browse.hidden_exited_session_ids.iter().cloned().collect();
    if app.effects.hidden_exited.as_ref() != Some(&hidden) {
        ran(app);
        if let Some(listener) = &app.props.callbacks.on_hidden_exited_session_ids_change {
            listener(&hidden);
        }
        app.effects.hidden_exited = Some(hidden);
    }
    // Not in the TS: the tasks whose steps the Tasks view hides, kept across attaches.
    let steps = app.flows.tasks.collapsed_steps.clone();
    if app.effects.collapsed_task_steps.as_ref() != Some(&steps) {
        ran(app);
        if let Some(listener) = &app.props.callbacks.on_collapsed_task_steps_change {
            listener(&steps);
        }
        app.effects.collapsed_task_steps = Some(steps);
    }
}

/// TS: the mount effect (`[]`).
fn mount_effect(app: &mut App) {
    if app.effects.mounted {
        return;
    }
    app.effects.mounted = true;
    ran(app);
    load_scroll_sensitivity(app);
}

/// TS: the `[activeTab]` effect: remember it for the selected session, report it, and leave the focus modes bound to
/// another tab.
fn active_tab_effect(app: &mut App) {
    let tab = app.browse.active_tab;
    if app.effects.active_tab == Some(tab) {
        return;
    }
    app.effects.active_tab = Some(tab);
    ran(app);
    if let Some(session_id) = app.selected_id_ref().filter(|id| !id.is_empty()) {
        app.browse.session_tabs.insert(session_id.clone(), tab);
        if let Some(listener) = &app.props.callbacks.on_session_tab_change {
            listener(&session_id, tab);
        }
    }
    if let Some(listener) = &app.props.callbacks.on_active_tab_change {
        listener(tab);
    }
    if tab != RightPaneTab::Preview {
        app.preview_focus.scroll_offset = 0;
        if app.mode == Mode::PreviewFocus {
            app.mode = Mode::Browse;
        }
    }
    if tab != RightPaneTab::Terminal && app.mode == Mode::TerminalFocus {
        app.mode = Mode::Browse;
    }
    if tab != RightPaneTab::Notes && app.mode == Mode::NotesFocus {
        app.mode = Mode::Browse;
    }
    if tab != RightPaneTab::Git && app.mode == Mode::ChangesFocus {
        app.mode = Mode::Browse;
    }
}

/// TS: `[shouldAnimateStatus]`: the spinner advances every 120 ms while a session works.
fn spinner_effect(app: &mut App) {
    let animate = animates(app);
    if app.effects.animating == Some(animate) {
        return;
    }
    app.effects.animating = Some(animate);
    ran(app);
    app.timers.spinner = animate.then(|| {
        app.connection.dispatch.every(SPINNER_PERIOD, |app: &mut App| {
            app.browse.spinner_index = (app.browse.spinner_index + 1) % SPINNER_FRAMES.len();
        })
    });
}

/// TS: the `[connectionEpoch, repoRoot]` effect: (re)connect, cleaning up the previous connection first.
fn connection_effect(app: &mut App) {
    let epoch = app.connection.epoch;
    if app.effects.connection == Some(epoch) {
        return;
    }
    if app.effects.connection.is_some() {
        disconnect(app);
    }
    app.effects.connection = Some(epoch);
    ran(app);
    connect(app);
}

/// TS: the connection effect's cleanup: cancelled, the reconnect timer cleared, its client closed.
fn disconnect(app: &mut App) {
    if let Some(cancelled) = app.effects.connection_cancel.take() {
        cancelled.set(true);
    }
    if let Some(task) = app.connection.connecting.take() {
        task.abort();
    }
    app.connection.reconnect_timer = None;
    if let Some(client) = app.connection.client.take() {
        client.close();
    }
}

/// TS: the subscription effect: connect (createLiveClient with [`live_client_handlers`]), subscribe to the
/// repository, then fetch agent versions and watch the task list; on failure or close, reconnect after 500 ms.
pub fn connect(app: &mut App) {
    let epoch = app.connection.epoch;
    let cancelled = Rc::new(Cell::new(false));
    app.effects.connection_cancel = Some(Rc::clone(&cancelled));
    let dispatch = app.connection.dispatch.clone();
    let handlers = live_client_handlers(&dispatch, epoch);
    let repo_root = app.props.repo_root.clone();
    let task = spawn(async move {
        let outcome: Result<(), DhError> = async {
            let client = Rc::new(create_live_client(handlers).await?);
            if cancelled.get() {
                client.close();
                return Ok(());
            }
            let shown = Rc::clone(&client);
            let flag = Rc::clone(&cancelled);
            dispatch.update(move |app: &mut App| {
                if !flag.get() {
                    app.connection.client = Some(shown);
                }
            });
            let sessions = client.subscribe(&repo_root).await;
            if cancelled.get() {
                client.close();
                return Ok(());
            }
            let sessions = sessions?;
            let flag = Rc::clone(&cancelled);
            dispatch.update(move |app: &mut App| {
                if flag.get() {
                    return;
                }
                app.set_repo_sessions(sessions);
                app.data.sessions_loaded = true;
                app.set_error(None);
            });
            // The daemon's cached versions; it looks the latest releases up in the background.
            let flag = Rc::clone(&cancelled);
            drop(
                dispatch.spawn(client.agent_versions(None), move |app: &mut App, result| {
                    if let Ok(versions) = result
                        && !flag.get()
                    {
                        app.data.agent_versions = Some(versions);
                    }
                }),
            );
            let flag = Rc::clone(&cancelled);
            drop(
                dispatch.spawn(client.watch_tasks(&repo_root), move |app: &mut App, result| {
                    if let Ok(doc) = result
                        && !flag.get()
                    {
                        app.set_tasks_doc(Some(doc));
                    }
                }),
            );
            let flag = Rc::clone(&cancelled);
            drop(dispatch.spawn(
                daemon_build_notice(client.ping(), |key: String| async move {
                    deckhand_sys::daemon_notice::claim_notice(&key).await
                }),
                move |app: &mut App, notice| {
                    if let Some(notice) = notice
                        && !flag.get()
                    {
                        app.show_notice(notice);
                    }
                },
            ));
            Ok(())
        }
        .await;
        if let Err(error) = outcome
            && !cancelled.get()
        {
            let flag = Rc::clone(&cancelled);
            dispatch.update(move |app: &mut App| {
                if flag.get() {
                    return;
                }
                app.set_error(Some(error.to_string()));
                schedule_reconnect(app);
            });
        }
    });
    app.connection.connecting = Some(task);
}

/// The footer notice for a compatible daemon of another build (`ping`: the daemon's answer), once per daemon:
/// `claim` records the notice key (`deckhand_sys::daemon_notice::claim_notice`) and says whether it is new. A daemon
/// that does not report its pid is keyed by the pid in `daemon.pid`.
pub async fn daemon_build_notice<C, F>(
    ping: impl Future<Output = Result<PingResult, DhError>>,
    claim: C,
) -> Option<String>
where
    C: FnOnce(String) -> F,
    F: Future<Output = bool>,
{
    let ping = ping.await.ok()?;
    let notice = update_notice(BUILD_VERSION, ping.build.as_deref())?;
    let pid = match ping.pid {
        Some(pid) => Some(pid),
        None => read_daemon_pid().await.and_then(|pid| u32::try_from(pid).ok()),
    };
    let key = notice_key(pid, ping.build.as_deref(), BUILD_VERSION);
    claim(key).await.then_some(notice)
}

/// TS: the createLiveClient handlers: each daemon push becomes an update of the app (records for another
/// selection are dropped).
pub fn live_client_handlers(dispatch: &Dispatch<App>, epoch: u64) -> LiveClientHandlers {
    let on = || dispatch.clone();
    let session_updated = on();
    let session_removed = on();
    let preview_updated = on();
    let terminal_updated = on();
    let changes_updated = on();
    let dev_updated = on();
    let action_updated = on();
    let versions_updated = on();
    let tasks_updated = on();
    let error = on();
    let close = on();
    LiveClientHandlers {
        // Sessions of the repository's other checkouts too (the board's); `sessions` keeps the sidebar to this one.
        on_session_updated: Some(Box::new(move |session: SessionRecord| {
            session_updated.update(move |app: &mut App| app.upsert(session));
        })),
        on_session_removed: Some(Box::new(move |session_id: String| {
            session_removed.update(move |app: &mut App| {
                app.data.repo_sessions.retain(|session| session.id != session_id);
                if app.selected_id_ref().as_deref() == Some(session_id.as_str()) {
                    app.reset_panes();
                }
            });
        })),
        on_preview_updated: Some(Box::new(move |next: PreviewRecord| {
            preview_updated.update(move |app: &mut App| {
                let selected = app.selected_id_ref();
                if next.session_id.is_some() && next.session_id != selected {
                    return;
                }
                if next.session_id.is_none() && selected.is_some() {
                    return;
                }
                let max = next.max_scroll_offset;
                app.panes.preview = next;
                if let Some(max) = max {
                    app.preview_focus.scroll_offset = app.preview_focus.scroll_offset.min(max);
                }
            });
        })),
        on_terminal_updated: Some(Box::new(move |next| {
            terminal_updated.update(move |app: &mut App| {
                let selected = app.selected_id_ref();
                if next.session_id.is_some() && next.session_id != selected {
                    return;
                }
                if next.session_id.is_none() && selected.is_some() {
                    return;
                }
                app.panes.terminal = next;
            });
        })),
        on_changes_updated: Some(Box::new(move |next| {
            changes_updated.update(move |app: &mut App| {
                if next.session_id == app.selected_id_ref() {
                    app.panes.changes = next;
                }
            });
        })),
        on_dev_updated: Some(Box::new(move |next| {
            dev_updated.update(move |app: &mut App| apply_dev_update(app, next));
        })),
        on_action_updated: Some(Box::new(move |next| {
            action_updated.update(move |app: &mut App| {
                if next.session_id == app.selected_id_ref() {
                    app.panes.action = next;
                }
            });
        })),
        on_agent_versions_updated: Some(Box::new(move |versions| {
            versions_updated.update(move |app: &mut App| app.data.agent_versions = Some(versions));
        })),
        on_tasks_updated: Some(Box::new(move |next| {
            tasks_updated.update(move |app: &mut App| {
                let keep = app
                    .data
                    .tasks_doc
                    .as_ref()
                    .is_some_and(|current| current.key != next.key);
                if !keep {
                    app.set_tasks_doc(Some(next));
                }
            });
        })),
        on_error: Some(Box::new(move |next_error: DhError| {
            error.update(move |app: &mut App| app.set_error(Some(next_error.to_string())));
        })),
        on_close: Some(Box::new(move || {
            close.update(move |app: &mut App| {
                // Only the current connection's close reconnects (an older client was closed by its cleanup).
                if app.connection.epoch == epoch {
                    app.connection.client = None;
                    schedule_reconnect(app);
                }
            });
        })),
    }
}

/// TS: scheduleReconnect: bump the connection epoch after 500 ms (once).
pub fn schedule_reconnect(app: &mut App) {
    if app.connection.reconnect_timer.is_some() {
        return;
    }
    app.connection.reconnect_timer = Some(
        app.connection
            .dispatch
            .after(RECONNECT_DELAY, |app: &mut App| app.connection.epoch += 1),
    );
}

/// TS: the selection effect: until sessions load keep the persisted selection; then keep a visible one, else the
/// first visible session (a filter matching nothing keeps it).
pub fn sync_selection(app: &mut App) {
    let derived = app.derived();
    selection_effect(app, &derived);
}

fn selection_effect(app: &mut App, derived: &Derived) {
    let loaded = app.data.sessions_loaded;
    let visible: Vec<String> = derived.visible.iter().map(|session| session.id.clone()).collect();
    let deps = (derived.sessions.clone(), visible, loaded);
    if app.effects.selection_sync.as_ref() == Some(&deps) {
        return;
    }
    app.effects.selection_sync = Some(deps);
    ran(app);
    // Sessions arrive asynchronously; until then keep the persisted (or pre-attach) selection.
    if !loaded {
        return;
    }
    let current = app.browse.selected_id.clone().filter(|id| !id.is_empty());
    let next = match current {
        Some(id) if derived.visible.iter().any(|session| session.id == id) => Some(id),
        // A filter/search that temporarily matches nothing keeps the selection.
        current if derived.visible.is_empty() => {
            current.filter(|id| derived.sessions.iter().any(|session| &session.id == id))
        }
        _ => derived.visible.first().map(|session| session.id.clone()),
    };
    app.browse.selected_id = next;
}

/// TS: the pane watch effects (watch-preview always; terminal/action, git and dev while their tab is shown;
/// watching Changes stops when Git is left).
pub fn sync_watchers(app: &mut App) {
    let derived = app.derived();
    watch_effects(app, &derived);
}

/// TS `selectedPaneScope`: the selected session's workspace and lifecycle.
fn pane_scope(selected: Option<&SessionRecord>) -> Option<String> {
    selected.map(|session| format!("{}\0{}", workspace_key(session).unwrap_or_default(), session.status))
}

fn watch_effects(app: &mut App, derived: &Derived) {
    let layout = app.layout();
    let base = PaneWatchDeps {
        epoch: client_key(app),
        session_id: app.browse.selected_id.clone(),
        cols: layout.preview_cols,
        rows: layout.preview_rows,
        scroll_offset: 0,
        pane_scope: None,
        active: true,
        interactive: false,
        typing_shell: None,
    };
    let scope = pane_scope(derived.selected.as_ref());
    let tab = app.browse.active_tab;
    watch_preview(
        app,
        PaneWatchDeps {
            scroll_offset: app.preview_focus.scroll_offset,
            interactive: app.mode == Mode::PreviewFocus,
            ..base.clone()
        },
    );
    let pane = PaneWatchDeps {
        pane_scope: scope.clone(),
        active: tab == RightPaneTab::Terminal,
        ..base.clone()
    };
    watch_terminal(
        app,
        PaneWatchDeps {
            typing_shell: (app.mode == Mode::TerminalFocus)
                .then_some(app.preview_focus.shell)
                .flatten(),
            ..pane.clone()
        },
    );
    watch_action(app, pane);
    watch_changes(
        app,
        PaneWatchDeps {
            cols: 0,
            rows: 0,
            pane_scope: scope.clone(),
            active: tab == RightPaneTab::Git,
            ..base.clone()
        },
    );
    // Named Dev processes share the tab: each PTY is sized to its cell.
    let (dev_cols, dev_rows) = match dev_processes(&app.panes.dev) {
        Some(processes) => dev_cell_size(layout.pane_inner_width, layout.pane_inner_height, processes.len()),
        None => (base.cols, base.rows),
    };
    watch_dev(
        app,
        PaneWatchDeps {
            pane_scope: scope,
            active: tab == RightPaneTab::Dev,
            cols: dev_cols,
            rows: dev_rows,
            ..base
        },
    );
}

fn watch_preview(app: &mut App, deps: PaneWatchDeps) {
    let Some(generation) = app.effects.preview.restart(deps.clone()) else {
        return;
    };
    ran(app);
    let Some(client) = app.client() else {
        return;
    };
    let reply = client.watch_preview(
        deps.session_id.as_deref(),
        i64::from(deps.cols),
        i64::from(deps.rows),
        Some(deps.scroll_offset),
        deps.interactive,
    );
    let task = app.dispatch().spawn(reply, move |app: &mut App, result| {
        if app.effects.preview.generation != generation {
            return;
        }
        match result {
            Ok(next) => {
                if next.session_id.is_some() && next.session_id != deps.session_id {
                    return;
                }
                let max = next.max_scroll_offset;
                app.panes.preview = next;
                if let Some(max) = max {
                    app.preview_focus.scroll_offset = app.preview_focus.scroll_offset.min(max);
                }
            }
            Err(error) => app.set_error(Some(error.to_string())),
        }
    });
    app.effects.preview.task = Some(task);
}

fn watch_terminal(app: &mut App, deps: PaneWatchDeps) {
    let Some(generation) = app.effects.terminal.restart(deps.clone()) else {
        return;
    };
    ran(app);
    let Some(client) = app.client().filter(|_| deps.active) else {
        return;
    };
    let reply = client.watch_terminal(
        deps.session_id.as_deref(),
        i64::from(deps.cols),
        i64::from(deps.rows),
        deps.typing_shell,
    );
    let task = app.dispatch().spawn(reply, move |app: &mut App, result| {
        if app.effects.terminal.generation != generation {
            return;
        }
        match result {
            Ok(next) => {
                if next.session_id.is_some() && next.session_id != deps.session_id {
                    return;
                }
                app.panes.terminal = next;
            }
            Err(error) => app.set_error(Some(error.to_string())),
        }
    });
    app.effects.terminal.task = Some(task);
}

/// The Terminal tab also shows the workspace's last action (its state in the header, its output on v). Watching it
/// never starts one.
fn watch_action(app: &mut App, deps: PaneWatchDeps) {
    let Some(generation) = app.effects.action.restart(deps.clone()) else {
        return;
    };
    ran(app);
    let Some(client) = app.client().filter(|_| deps.active) else {
        return;
    };
    let reply = client.watch_action(deps.session_id.as_deref(), i64::from(deps.cols), i64::from(deps.rows));
    let task = app.dispatch().spawn(reply, move |app: &mut App, result| {
        if app.effects.action.generation != generation {
            return;
        }
        match result {
            Ok(next) => {
                if next.session_id == deps.session_id {
                    app.panes.action = next;
                }
            }
            Err(error) => app.set_error(Some(error.to_string())),
        }
    });
    app.effects.action.task = Some(task);
}

/// The Git tab shows the workspace's Changes (lazygit only on attach). The daemon polls a watched workspace, so
/// leaving the tab stops watching.
fn watch_changes(app: &mut App, deps: PaneWatchDeps) {
    let Some(generation) = app.effects.changes.restart(deps.clone()) else {
        return;
    };
    ran(app);
    let Some(client) = app.client() else {
        app.panes.changes_watched = false;
        return;
    };
    if !deps.active {
        if app.panes.changes_watched {
            app.panes.changes_watched = false;
            // Fire and forget: the request is written now; its reply is not needed.
            drop(client.watch_changes(None));
        }
        return;
    }
    app.panes.changes_watched = true;
    let reply = client.watch_changes(deps.session_id.as_deref());
    let task = app.dispatch().spawn(reply, move |app: &mut App, result| {
        if app.effects.changes.generation != generation {
            return;
        }
        match result {
            Ok(next) => {
                if next.session_id == deps.session_id {
                    app.panes.changes = next;
                }
            }
            Err(error) => app.set_error(Some(error.to_string())),
        }
    });
    app.effects.changes.task = Some(task);
}

/// A `dev-updated` event: the selected session's Dev pane (or the repository's, with none selected), counted so a start
/// reply already in flight knows it is older (`keep_newer_dev`).
pub(crate) fn apply_dev_update(app: &mut App, next: DevRecord) {
    let selected = app.selected_id_ref();
    if next.session_id.is_some() && next.session_id != selected {
        return;
    }
    if next.session_id.is_none() && selected.is_some() {
        return;
    }
    app.panes.dev = next;
    app.panes.dev_updates += 1;
}

fn watch_dev(app: &mut App, deps: PaneWatchDeps) {
    let Some(generation) = app.effects.dev.restart(deps.clone()) else {
        return;
    };
    ran(app);
    let Some(client) = app.client().filter(|_| deps.active) else {
        return;
    };
    let reply = client.watch_dev(deps.session_id.as_deref(), i64::from(deps.cols), i64::from(deps.rows));
    let task = app.dispatch().spawn(reply, move |app: &mut App, result| {
        if app.effects.dev.generation != generation {
            return;
        }
        match result {
            Ok(next) => {
                if next.session_id.is_some() && next.session_id != deps.session_id {
                    return;
                }
                app.panes.dev = next;
            }
            Err(error) => app.set_error(Some(error.to_string())),
        }
    });
    app.effects.dev.task = Some(task);
}

/// TS: the `[selectedSession]` effect: records of another session are dropped; the preview keeps its content only
/// for the same session (an exited one shows its last preview).
fn selected_session_effect(app: &mut App, derived: &Derived) {
    let deps = SelectedSessionDeps {
        session: derived.selected.clone(),
    };
    if app.effects.selected_session.as_ref() == Some(&deps) {
        return;
    }
    app.effects.selected_session = Some(deps);
    ran(app);
    let Some(selected) = derived.selected.as_ref() else {
        app.reset_panes();
        return;
    };
    let mine = |id: &Option<String>| id.as_deref() == Some(selected.id.as_str());
    if !mine(&app.panes.terminal.session_id) {
        app.panes.terminal = empty_pane();
    }
    if !mine(&app.panes.changes.session_id) {
        app.panes.changes = deckhand_core::changes_model::empty_changes();
    }
    if !mine(&app.panes.dev.session_id) {
        app.panes.dev = empty_pane();
    }
    if !mine(&app.panes.action.session_id) {
        app.panes.action = empty_pane();
    }
    let current = &app.panes.preview;
    let same_session = mine(&current.session_id);
    let content = if selected.status == SessionStatus::Exited {
        selected.last_preview.clone().unwrap_or_else(|| current.content.clone())
    } else if same_session {
        current.content.clone()
    } else {
        String::new()
    };
    app.panes.preview = PreviewRecord {
        session_id: Some(selected.id.clone()),
        content,
        live: same_session && current.live,
        status: Some(selected.status),
        agent_status: selected.agent_status,
        scroll_offset: None,
        max_scroll_offset: None,
        screen: None,
    };
}

/// TS: the timer effects: status/error auto-hide, the spinner interval, numeric selection, the age clock.
pub fn sync_timers(app: &mut App) {
    let derived = app.derived();
    status_message_effect(app);
    error_effect(app);
    spinner_effect(app);
    numeric_selection_effect(app);
    clock_effect(app, &derived);
}

/// TS: digits typed for a session number are confirmed 300 ms after the last one.
fn numeric_selection_effect(app: &mut App) {
    let value = app.browse.numeric_selection.clone();
    if app.effects.numeric_selection.as_ref() == Some(&value) {
        return;
    }
    app.effects.numeric_selection = Some(value.clone());
    ran(app);
    app.timers.numeric_selection = (!value.is_empty()).then(|| {
        app.connection
            .dispatch
            .after(NUMERIC_SELECTION_DELAY, move |app: &mut App| {
                crate::app::modes::browse::confirm_numeric_selection(app, &value);
            })
    });
}

/// Node's `setTimeout` delay: below 1 ms (or not a number) is 1 ms.
fn timeout_delay(ms: f64) -> Duration {
    if (1.0..=2_147_483_647.0).contains(&ms) {
        Duration::from_millis(ms as u64)
    } else {
        Duration::from_millis(1)
    }
}

/// TS: the sidebar details show the selected session's age; while nothing else re-renders (no spinner), one render
/// exactly when the shown age (or the done marker's) would change keeps it current.
fn clock_effect(app: &mut App, derived: &Derived) {
    let selected = derived.selected.as_ref();
    let since = selected.and_then(status_since).map(str::to_owned);
    let done_at = selected.and_then(|session| session.done_at.clone());
    let animate = animates(app);
    let deps = (since.clone(), done_at.clone(), app.browse.clock_tick, animate);
    if app.effects.clock.as_ref() == Some(&deps) {
        return;
    }
    app.effects.clock = Some(deps);
    ran(app);
    app.timers.clock = None;
    let ages: Vec<i64> = [since, done_at]
        .iter()
        .flatten()
        .filter(|at| !at.is_empty())
        .filter_map(|at| date::parse(at))
        .collect();
    if animate || ages.is_empty() {
        return;
    }
    let now = date::now_ms();
    let delay = ages
        .iter()
        .map(|at| ms_until_age_changes((now - at) as f64))
        .fold(f64::INFINITY, f64::min);
    app.timers.clock = Some(
        app.connection
            .dispatch
            .after(timeout_delay(delay), |app: &mut App| app.browse.clock_tick += 1),
    );
}

/// TS: `[filteredWorktrees.length]`: the existing-worktree picker's index stays on the list.
fn worktree_index_effect(app: &mut App) {
    let count = if app.create.worktrees.is_empty() {
        0
    } else {
        crate::app::modes::create::filtered_worktrees(&app.create.worktrees, &app.create.worktree_query).len()
    };
    if app.effects.worktree_count == Some(count) {
        return;
    }
    app.effects.worktree_count = Some(count);
    ran(app);
    app.create.worktree_index = app.create.worktree_index.min(count.saturating_sub(1));
}

/// TS: the new worktree's base branches, loaded once the form shows a new worktree.
fn branches_effect(app: &mut App) {
    let active = app.mode == Mode::EnterName
        && app.create.worktree_mode == deckhand_protocol::data::WorktreeMode::New
        && app.create.branch_list.is_none()
        && app.connection.client.is_some();
    if !active {
        if app.effects.branches.deps.is_some() {
            app.effects.branches.cancel();
            app.effects.branches.deps = None;
            ran(app);
        }
        return;
    }
    let parent_cwd = app
        .create
        .parent_id
        .as_ref()
        .and_then(|id| app.data.repo_sessions.iter().find(|session| &session.id == id))
        .map(|session| session.cwd.clone())
        .unwrap_or_else(|| app.props.cwd.clone());
    let deps = (client_key(app), parent_cwd);
    if app.effects.branches.deps.as_ref() == Some(&deps) {
        return;
    }
    app.effects.branches.cancel();
    ran(app);
    crate::app::modes::create::load_branches(app);
    app.effects.branches.deps = Some(deps);
}

/// TS: the terminal-mode effects: bracketed paste in notes focus (and, not in the TS, in preview focus, so a paste
/// reaches the agent as one), mouse reporting in preview focus (escape sequences written to stdout, undone when the
/// mode ends).
pub fn sync_terminal_modes(app: &mut App, out: &mut dyn std::io::Write) {
    let mode = app.mode;
    if app.effects.mode == Some(mode) {
        return;
    }
    app.effects.mode = Some(mode);
    ran(app);
    // React runs every cleanup before the new setups.
    let mut text = String::new();
    let pastes = matches!(mode, Mode::NotesFocus | Mode::PreviewFocus | Mode::TerminalFocus);
    if app.effects.paste_on && !pastes {
        app.effects.paste_on = false;
        text.push_str(PASTE_OFF);
    }
    if app.effects.mouse_on && mode != Mode::PreviewFocus {
        app.effects.mouse_on = false;
        text.push_str(MOUSE_OFF);
    }
    // Bracketed paste only on a terminal (TS `process.stdout.isTTY`).
    let tty = !app.effects.stdout_modes || std::io::IsTerminal::is_terminal(&std::io::stdout());
    if pastes && !app.effects.paste_on && tty {
        app.effects.paste_on = true;
        text.push_str(PASTE_ON);
    }
    if mode == Mode::PreviewFocus && !app.effects.mouse_on {
        app.effects.mouse_on = true;
        text.push_str(MOUSE_ON);
    }
    if !text.is_empty() {
        let _ = out.write_all(text.as_bytes());
        let _ = out.flush();
    }
}

/// XTWINOPS: save the terminal's title on its stack, and restore it.
const TITLE_PUSH: &str = "\u{1b}[22;0t";
const TITLE_POP: &str = "\u{1b}[23;0t";

/// Not in the TS: the terminal's title, `deckhand · <repository>` (just `deckhand` without one).
pub fn terminal_title(repo_root: &str) -> String {
    match repo_root.trim_end_matches('/').rsplit('/').next() {
        Some(repo) if !repo.is_empty() => format!("deckhand · {repo}"),
        _ => "deckhand".to_owned(),
    }
}

/// Not in the TS: writes [`terminal_title`] when it changed; the first write saves the terminal's own title, which
/// [`unmount`] gives back. (An attach sets its own title, then `deckhand`, and the UI that comes back sets this one.)
pub fn sync_terminal_title(app: &mut App, out: &mut dyn std::io::Write) {
    let title = terminal_title(&app.props.repo_root);
    if app.effects.title.as_deref() == Some(title.as_str()) {
        return;
    }
    ran(app);
    let mut text = String::new();
    if app.effects.title.is_none() {
        text.push_str(TITLE_PUSH);
    }
    let shown = sanitize_terminal_title(&title);
    text.push_str(&format!("\u{1b}]0;{shown}\u{7}\u{1b}]2;{shown}\u{7}"));
    app.effects.title = Some(title);
    let _ = out.write_all(text.as_bytes());
    let _ = out.flush();
}

/// TS: the effects that report state to cli.ts (selected id, tabs, collapse sets, visibility, terminal view).
pub fn report_state(app: &mut App) {
    terminal_view_effect(app);
    visibility_effect(app);
    session_view_effect(app);
    selected_id_effect(app);
    collapsed_effect(app);
    active_tab_effect(app);
}

/// TS: the mount effect: loads the wheel speed from the app config (`scroll_speed`; the TS read
/// `attach_scroll_sensitivity`) and, not in the TS, `sidebar_width_percent`.
pub fn load_scroll_sensitivity(app: &mut App) {
    let task = app.dispatch().spawn(load_app_config(), |app: &mut App, result| {
        app.preview_focus.scroll_sensitivity = match &result {
            Ok(config) => normalize_scroll_sensitivity(config.scroll_speed.map(|value| value.0)),
            Err(_) => crate::app::state::DEFAULT_SCROLL_SENSITIVITY,
        };
        let percent = result.ok().and_then(|config| config.sidebar_width_percent);
        app.browse.sidebar_percent = percent.map(|value| normalize_sidebar_percent(Some(value.0)));
    });
    drop(task);
}

/// TS: every effect's cleanup when App unmounts: the connection closed, requests and timers cancelled, terminal
/// modes undone.
pub fn unmount(app: &mut App) {
    disconnect(app);
    for watch in [
        &mut app.effects.preview,
        &mut app.effects.terminal,
        &mut app.effects.action,
        &mut app.effects.changes,
        &mut app.effects.dev,
    ] {
        watch.cancel();
    }
    app.effects.branches.cancel();
    app.timers = AppTimers::default();
    app.messages.error_timer = None;
    app.messages.status_timer = None;
    let mut text = String::new();
    if std::mem::take(&mut app.effects.paste_on) {
        text.push_str(PASTE_OFF);
    }
    if std::mem::take(&mut app.effects.mouse_on) {
        text.push_str(MOUSE_OFF);
    }
    if app.effects.title.take().is_some() {
        text.push_str(TITLE_POP);
    }
    if app.effects.stdout_modes && !text.is_empty() {
        let mut stdout = std::io::stdout();
        let _ = stdout.write_all(text.as_bytes());
        let _ = stdout.flush();
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;
    use crate::app::state::{AppCallbacks, AppProps};
    use crate::app::test_support::{app, session};
    use deckhand_protocol::data::SessionAttention;

    fn attention(state: AttentionState) -> SessionAttention {
        serde_json::from_value(
            serde_json::json!({"state": state.as_str(), "event": "e", "at": "2026-01-01T00:00:00.000Z"}),
        )
        .unwrap_or_else(|error| panic!("{error}"))
    }

    #[test]
    fn the_spinner_runs_for_starting_and_working_sessions() {
        let starting = session("a", "A", SessionStatus::Starting);
        assert!(should_animate_status(std::slice::from_ref(&starting)));
        let mut running = session("b", "B", SessionStatus::Running);
        assert!(!session_animates(SessionStatus::Running, &running));
        running.agent_status = Some(AgentActivityStatus::Active);
        assert!(session_animates(SessionStatus::Running, &running));
        running.attention = Some(attention(AttentionState::NeedsInput));
        assert!(!session_animates(SessionStatus::Running, &running));
        running.attention = Some(attention(AttentionState::Working));
        running.agent_status = Some(AgentActivityStatus::Idle);
        assert!(session_animates(SessionStatus::Running, &running));
        assert!(!session_animates(SessionStatus::Exited, &running));
        assert!(!should_animate_status(&[]));
    }

    #[test]
    fn a_spinner_is_drawn_only_while_it_animates() {
        // A read reply whose screen changed drew a spinner frame that never moved.
        let mut cases = 0;
        for status in [SessionStatus::Starting, SessionStatus::Running, SessionStatus::Exited] {
            for agent_status in [
                None,
                Some(AgentActivityStatus::Active),
                Some(AgentActivityStatus::Idle),
                Some(AgentActivityStatus::Unknown),
            ] {
                let states = [
                    None,
                    Some(AttentionState::Working),
                    Some(AttentionState::NeedsInput),
                    Some(AttentionState::ResponseEnded),
                    Some(AttentionState::Failed),
                    Some(AttentionState::Limited),
                    Some(AttentionState::Unknown),
                ];
                for state in states {
                    for reply_read in [None, Some(true)] {
                        let mut session = session("a", "A", status);
                        session.agent_status = agent_status;
                        session.attention = state.map(attention);
                        session.reply_read = reply_read;
                        let spinning = deckhand_core::display::status_glyph(&session, "⠙") == "⠙";
                        assert_eq!(
                            spinning,
                            session_animates(status, &session),
                            "{status:?} {agent_status:?} {state:?} {reply_read:?}"
                        );
                        cases += 1;
                    }
                }
            }
        }
        assert_eq!(cases, 168);
    }

    /// Runs `body` inside a LocalSet (effects start timers).
    fn local(body: impl std::future::Future<Output = ()>) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap_or_else(|error| panic!("{error}"));
        tokio::task::LocalSet::new().block_on(&runtime, body);
    }

    #[test]
    fn selection_waits_for_sessions_then_keeps_a_visible_one() {
        local(async {
            let (mut app, _rx) = app(AppProps {
                initial_selected_id: Some("gone".to_owned()),
                ..AppProps::default()
            });
            app.data.repo_sessions = vec![
                session("a", "A", SessionStatus::Running),
                session("b", "B", SessionStatus::Running),
            ];
            run_effects(&mut app);
            assert_eq!(app.browse.selected_id.as_deref(), Some("gone"));
            app.data.sessions_loaded = true;
            run_effects(&mut app);
            assert_eq!(app.browse.selected_id.as_deref(), Some("a"));
            app.set_selected_id(Some("b".to_owned()));
            run_effects(&mut app);
            assert_eq!(app.browse.selected_id.as_deref(), Some("b"));
            // A search matching nothing keeps the selection.
            app.browse.session_query = "zzz".to_owned();
            run_effects(&mut app);
            assert_eq!(app.browse.selected_id.as_deref(), Some("b"));
            app.browse.session_query.clear();
            app.data.repo_sessions.retain(|session| session.id != "b");
            run_effects(&mut app);
            assert_eq!(app.browse.selected_id.as_deref(), Some("a"));
        });
    }

    #[test]
    fn selection_and_tab_effects_report_and_remember_tabs() {
        local(async {
            let log: Rc<RefCell<Vec<String>>> = Rc::default();
            let (selected, tabs, active) = (Rc::clone(&log), Rc::clone(&log), Rc::clone(&log));
            let callbacks = AppCallbacks {
                on_selected_id_change: Some(Box::new(move |id| selected.borrow_mut().push(format!("id {id:?}")))),
                on_session_tab_change: Some(Box::new(move |id, tab| {
                    tabs.borrow_mut().push(format!("tab {id} {tab}"))
                })),
                on_active_tab_change: Some(Box::new(move |tab| active.borrow_mut().push(format!("active {tab}")))),
                ..AppCallbacks::default()
            };
            let mut tabs_by_session = indexmap::IndexMap::new();
            tabs_by_session.insert("b".to_owned(), RightPaneTab::Terminal);
            let (mut app, _rx) = app(AppProps {
                initial_selected_id: Some("a".to_owned()),
                initial_active_tab: Some(RightPaneTab::Dev),
                initial_session_tabs: Some(tabs_by_session),
                callbacks,
                ..AppProps::default()
            });
            app.data.repo_sessions = vec![
                session("a", "A", SessionStatus::Running),
                session("b", "B", SessionStatus::Running),
            ];
            app.data.sessions_loaded = true;
            run_effects(&mut app);
            assert_eq!(app.browse.active_tab, RightPaneTab::Dev);
            assert_eq!(log.borrow().as_slice(), ["id Some(\"a\")", "tab a dev", "active dev"]);
            log.borrow_mut().clear();
            app.preview_focus.scroll_offset = 4;
            app.set_selected_id(Some("b".to_owned()));
            run_effects(&mut app);
            assert_eq!(app.browse.active_tab, RightPaneTab::Terminal);
            assert_eq!(app.preview_focus.scroll_offset, 0);
            assert_eq!(
                log.borrow().as_slice(),
                ["id Some(\"b\")", "tab b terminal", "active terminal"]
            );
            // Leaving a tab ends the focus mode bound to it.
            app.set_mode(Mode::ChangesFocus);
            app.set_active_tab(RightPaneTab::Notes);
            run_effects(&mut app);
            assert_eq!(app.mode, Mode::Browse);
            assert_eq!(app.browse.session_tabs.get("b"), Some(&RightPaneTab::Notes));
        });
    }

    #[test]
    fn the_selected_session_resets_pane_records_of_others() {
        local(async {
            let (mut app, _rx) = app(AppProps {
                initial_selected_id: Some("a".to_owned()),
                ..AppProps::default()
            });
            let mut exited = session("x", "X", SessionStatus::Exited);
            exited.last_preview = Some("last words".to_owned());
            app.data.repo_sessions = vec![session("a", "A", SessionStatus::Running), exited];
            app.data.sessions_loaded = true;
            app.browse.session_filter = SessionFilter::All;
            app.panes.terminal.session_id = Some("other".to_owned());
            app.panes.terminal.content = "old".to_owned();
            app.panes.preview.content = "stale".to_owned();
            run_effects(&mut app);
            assert_eq!(app.browse.selected_id.as_deref(), Some("a"));
            assert_eq!(app.panes.terminal.content, "");
            assert_eq!(app.panes.preview.session_id.as_deref(), Some("a"));
            assert_eq!(app.panes.preview.content, "");
            assert_eq!(app.panes.preview.status, Some(SessionStatus::Running));
            app.set_selected_id(Some("x".to_owned()));
            run_effects(&mut app);
            assert_eq!(app.panes.preview.content, "last words");
            app.data.repo_sessions.clear();
            run_effects(&mut app);
            assert_eq!(app.panes.preview.session_id, None);
        });
    }

    #[test]
    fn messages_hide_after_their_delay_unless_replaced() {
        local(async {
            let (mut app, _rx) = app(AppProps::default());
            app.set_status(Some("saved".to_owned()));
            app.set_error(Some("boom".to_owned()));
            run_effects(&mut app);
            assert!(
                app.messages
                    .status_timer
                    .as_ref()
                    .is_some_and(|timer| timer.is_pending())
            );
            assert!(
                app.messages
                    .error_timer
                    .as_ref()
                    .is_some_and(|timer| timer.is_pending())
            );
            // No message, no timer; a timer only hides the message it was set for.
            app.set_status(None);
            run_effects(&mut app);
            assert!(app.messages.status_timer.is_none());
            hide_error(&mut app, "older");
            assert_eq!(app.messages.error.as_deref(), Some("boom"));
            hide_error(&mut app, "boom");
            assert_eq!(app.messages.error, None);
            app.set_status(Some("again".to_owned()));
            hide_status(&mut app, "saved");
            assert_eq!(app.messages.status_message.as_deref(), Some("again"));
            hide_status(&mut app, "again");
            assert_eq!(app.messages.status_message, None);
        });
    }

    #[test]
    fn terminal_modes_follow_the_focus_modes() {
        local(async {
            let (mut app, _rx) = app(AppProps::default());
            let mut out = Vec::new();
            sync_terminal_modes(&mut app, &mut out);
            assert!(out.is_empty());
            // Typing to the agent: pastes stay whole, and the wheel comes as mouse reports.
            app.set_mode(Mode::PreviewFocus);
            sync_terminal_modes(&mut app, &mut out);
            assert_eq!(String::from_utf8_lossy(&out), format!("{PASTE_ON}{MOUSE_ON}"));
            out.clear();
            app.set_mode(Mode::NotesFocus);
            sync_terminal_modes(&mut app, &mut out);
            assert_eq!(String::from_utf8_lossy(&out), MOUSE_OFF);
            out.clear();
            app.set_mode(Mode::Browse);
            sync_terminal_modes(&mut app, &mut out);
            assert_eq!(String::from_utf8_lossy(&out), PASTE_OFF);
        });
    }

    #[test]
    fn the_terminal_title_names_the_repository() {
        assert_eq!(terminal_title("/code/demo/"), "deckhand · demo");
        assert_eq!(terminal_title(""), "deckhand");
    }

    #[test]
    fn the_terminal_title_is_written_once_and_the_old_one_restored() {
        local(async {
            let (mut app, _rx) = app(AppProps::default());
            let mut out = Vec::new();
            sync_terminal_title(&mut app, &mut out);
            assert_eq!(
                String::from_utf8_lossy(&out),
                format!("{TITLE_PUSH}\u{1b}]0;deckhand · repo\u{7}\u{1b}]2;deckhand · repo\u{7}")
            );
            out.clear();
            sync_terminal_title(&mut app, &mut out);
            assert!(out.is_empty(), "unchanged");
            unmount(&mut app);
            assert_eq!(app.effects.title, None);
        });
    }

    #[test]
    fn daemon_events_update_the_app() {
        local(async {
            let (mut app, mut rx) = app(AppProps {
                initial_selected_id: Some("a".to_owned()),
                ..AppProps::default()
            });
            let handlers = live_client_handlers(&app.connection.dispatch.clone(), 0);
            let call = |handler: &Option<Box<dyn Fn(PreviewRecord)>>, record: PreviewRecord| {
                if let Some(handler) = handler {
                    handler(record);
                }
            };
            if let Some(handler) = &handlers.on_session_updated {
                handler(session("a", "A", SessionStatus::Running));
            }
            let mut preview = crate::app::state::empty_preview();
            preview.session_id = Some("a".to_owned());
            preview.content = "hello".to_owned();
            preview.max_scroll_offset = Some(2);
            call(&handlers.on_preview_updated, preview.clone());
            preview.session_id = Some("b".to_owned());
            preview.content = "other".to_owned();
            call(&handlers.on_preview_updated, preview);
            if let Some(handler) = &handlers.on_error {
                handler(DhError::msg("socket broke"));
            }
            if let Some(handler) = &handlers.on_close {
                handler();
            }
            app.preview_focus.scroll_offset = 5;
            for _ in 0..5 {
                let update = rx.recv().await.unwrap_or_else(|| panic!("update"));
                update(&mut app);
            }
            assert_eq!(app.data.repo_sessions.len(), 1);
            assert_eq!(app.panes.preview.content, "hello");
            assert_eq!(app.preview_focus.scroll_offset, 2);
            assert_eq!(app.messages.error.as_deref(), Some("socket broke"));
            assert!(app.connection.reconnect_timer.is_some());
            if let Some(handler) = &handlers.on_session_removed {
                handler("a".to_owned());
            }
            let update = rx.recv().await.unwrap_or_else(|| panic!("update"));
            update(&mut app);
            assert!(app.data.repo_sessions.is_empty());
            assert_eq!(app.panes.preview.content, "");
        });
    }

    #[test]
    fn the_clock_ticks_when_the_shown_age_changes() {
        local(async {
            let (mut app, _rx) = app(AppProps::default());
            app.data.repo_sessions = vec![session("a", "A", SessionStatus::Exited)];
            app.data.sessions_loaded = true;
            app.browse.session_filter = SessionFilter::All;
            run_effects(&mut app);
            assert!(app.timers.clock.is_some());
            assert!(app.timers.spinner.is_none());
            app.data.repo_sessions = vec![session("a", "A", SessionStatus::Starting)];
            run_effects(&mut app);
            assert!(app.timers.clock.is_none());
            assert!(app.timers.spinner.is_some());
            unmount(&mut app);
            assert!(app.timers.spinner.is_none());
        });
    }

    #[test]
    fn the_spinner_runs_for_another_checkouts_working_session() {
        // The Tasks view drew this session's spinner, which never moved: the sidebar does not list it.
        local(async {
            let (mut app, _rx) = app(AppProps {
                repo_root: "/repos/main".into(),
                ..AppProps::default()
            });
            let mut other = session("a", "A", SessionStatus::Running);
            other.repo_root = "/repos/clone".into();
            other.cwd = "/repos/clone".into();
            other.attention = Some(attention(AttentionState::Working));
            app.data.repo_sessions = vec![other];
            app.data.sessions_loaded = true;
            run_effects(&mut app);
            assert!(app.sessions().is_empty());
            assert!(app.timers.spinner.is_some());
            unmount(&mut app);
        });
    }

    fn ping(build: Option<&str>, pid: Option<u32>) -> PingResult {
        PingResult {
            ok: true,
            version: Some(deckhand_protocol::PROTOCOL_VERSION),
            build: build.map(str::to_owned),
            pid,
            ..PingResult::default()
        }
    }

    #[test]
    fn the_daemon_notice_shows_once_per_daemon_and_build() {
        local(async {
            let dir = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
            let marker = dir.path().join("daemon-notice");
            let notice = |answer: Result<PingResult, DhError>| {
                let marker = marker.clone();
                daemon_build_notice(async move { answer }, move |key: String| async move {
                    deckhand_sys::daemon_notice::claim_notice_at(&marker, &key).await
                })
            };
            // The same build: nothing to say, nothing recorded.
            assert_eq!(notice(Ok(ping(Some(BUILD_VERSION), Some(7)))).await, None);
            assert!(!marker.exists());
            // An older daemon: shown once for that daemon process.
            let expected = update_notice(BUILD_VERSION, Some("0.0.1"));
            assert!(expected.is_some());
            assert_eq!(notice(Ok(ping(Some("0.0.1"), Some(7)))).await, expected);
            assert_eq!(notice(Ok(ping(Some("0.0.1"), Some(7)))).await, None);
            // Restarted (another pid) or replaced by another build: shown again, once.
            assert_eq!(notice(Ok(ping(Some("0.0.1"), Some(8)))).await, expected);
            assert_eq!(notice(Ok(ping(Some("0.0.1"), Some(8)))).await, None);
            let other = update_notice(BUILD_VERSION, Some("0.0.2"));
            assert_eq!(notice(Ok(ping(Some("0.0.2"), Some(8)))).await, other);
            // A daemon from before build reporting is older too; a failed ping says nothing.
            assert_eq!(
                notice(Ok(ping(None, Some(9)))).await,
                update_notice(BUILD_VERSION, None)
            );
            assert_eq!(notice(Err(DhError::msg("daemon connection closed"))).await, None);
        });
    }

    #[test]
    fn node_timeouts_are_at_least_one_millisecond() {
        assert_eq!(timeout_delay(0.2), Duration::from_millis(1));
        assert_eq!(timeout_delay(f64::NAN), Duration::from_millis(1));
        assert_eq!(timeout_delay(60_000.0), Duration::from_millis(60_000));
    }
}
