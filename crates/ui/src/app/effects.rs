//! Port of app.tsx's effects: every `useEffect` of App, run after each batch of events (React's effect phase), and
//! the daemon subscription (`createLiveClient` with its handlers, reconnecting 500 ms after a close).
//!
//! Each effect keeps its last dependencies here; when they change, the previous run is cleaned up (its request
//! aborted, its timer dropped, as TS's `cancelled` flags and cleanups do) and it runs again. Watch requests resolve
//! through the app's [`Dispatch`], so a late reply for another selection is dropped as in TS.

use deckhand_client::live::LiveClientHandlers;
use deckhand_protocol::data::{RightPaneTab, SessionRecord, SessionStatus};
use deckhand_protocol::records::SessionFilter;
use deckhand_sys::async_util::Task;

use crate::app::event_loop::Dispatch;
use crate::app::state::{App, Mode};
use crate::panes::terminal::TerminalView;

/// One effect's last dependencies and the work it started.
#[derive(Debug)]
pub struct Watch<D> {
    pub deps: Option<D>,
    pub task: Option<Task<()>>,
}

impl<D> Default for Watch<D> {
    fn default() -> Self {
        Watch { deps: None, task: None }
    }
}

/// The dependencies of a pane watch (watch-preview, -terminal, -action, -changes, -dev): which client, session and
/// size it asked for, and the workspace/lifecycle scope (TS `selectedPaneScope`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PaneWatchDeps {
    /// TS `client` identity: the connection epoch it was created in.
    pub epoch: u64,
    pub session_id: Option<String>,
    pub cols: u16,
    pub rows: u16,
    /// Preview only.
    pub scroll_offset: i64,
    pub pane_scope: Option<String>,
    /// The watching tab is shown (terminal, git, dev).
    pub active: bool,
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
    pub selected_id: Option<Option<String>>,
    pub active_tab: Option<RightPaneTab>,
    pub mode: Option<Mode>,
    pub terminal_view: Option<TerminalView>,
    pub visibility: Option<(SessionFilter, String)>,
    pub collapsed: Option<Vec<String>>,
    pub hidden_exited: Option<Vec<String>>,
    pub selected_session: Option<SelectedSessionDeps>,
    /// TS `shouldAnimateStatus` at the last run (the spinner interval).
    pub animating: Option<bool>,
    /// The visible list and load state the selection was last reconciled with.
    pub selection_sync: Option<(Vec<String>, bool)>,
    pub status_message: Option<Option<String>>,
    pub error: Option<Option<String>>,
    pub numeric_selection: Option<String>,
    /// The selected session's status-since and done-at the age timer was set for.
    pub clock: Option<(Option<String>, Option<String>, u64, bool)>,
    pub preview: Watch<PaneWatchDeps>,
    pub terminal: Watch<PaneWatchDeps>,
    pub action: Watch<PaneWatchDeps>,
    pub changes: Watch<PaneWatchDeps>,
    pub dev: Watch<PaneWatchDeps>,
    /// The new worktree's base branches (enter-name with a new worktree).
    pub branches: Watch<(u64, String)>,
    /// The connection epoch the subscription runs for.
    pub connection: Option<u64>,
}

/// TS `shouldAnimateStatus`: a session starting, or running and working.
pub fn should_animate_status(sessions: &[SessionRecord]) -> bool {
    let _ = sessions;
    todo!("A1")
}

/// TS: whether `session` counts as working for the spinner (`starting`, or running with attention `working`, or an
/// active agent with unknown attention).
pub fn session_animates(status: SessionStatus, session: &SessionRecord) -> bool {
    let _ = (status, session);
    todo!("A1")
}

/// Runs every effect whose dependencies changed (TS: React's effect phase after a render).
pub fn run_effects(app: &mut App) {
    let _ = app;
    todo!("A1")
}

/// TS: the subscription effect: connect (createLiveClient with [`live_client_handlers`]), subscribe to the
/// repository, then fetch agent versions and watch the task list; on failure or close, reconnect after 500 ms.
pub fn connect(app: &mut App) {
    let _ = app;
    todo!("A1")
}

/// TS: the createLiveClient handlers: each daemon push becomes an update of the app (records for another
/// selection are dropped).
pub fn live_client_handlers(dispatch: &Dispatch<App>, epoch: u64) -> LiveClientHandlers {
    let _ = (dispatch, epoch);
    todo!("A1")
}

/// TS: scheduleReconnect: bump the connection epoch after 500 ms (once).
pub fn schedule_reconnect(app: &mut App) {
    let _ = app;
    todo!("A1")
}

/// TS: the selection effect: until sessions load keep the persisted selection; then keep a visible one, else the
/// first visible session (a filter matching nothing keeps it).
pub fn sync_selection(app: &mut App) {
    let _ = app;
    todo!("A1")
}

/// TS: the pane watch effects (watch-preview always; terminal/action, git and dev while their tab is shown;
/// watching Changes stops when Git is left).
pub fn sync_watchers(app: &mut App) {
    let _ = app;
    todo!("A1")
}

/// TS: the timer effects: status/error auto-hide, the spinner interval, numeric selection, the age clock.
pub fn sync_timers(app: &mut App) {
    let _ = app;
    todo!("A1")
}

/// TS: the terminal-mode effects: bracketed paste in notes focus, mouse reporting in preview focus (escape
/// sequences written to stdout, undone when the mode ends).
pub fn sync_terminal_modes(app: &mut App, out: &mut dyn std::io::Write) {
    let _ = (app, out);
    todo!("A1")
}

/// TS: the effects that report state to cli.ts (selected id, tabs, collapse sets, visibility, terminal view).
pub fn report_state(app: &mut App) {
    let _ = app;
    todo!("A1")
}

/// TS: the mount effect: loads `attach_scroll_sensitivity` from the app config.
pub fn load_scroll_sensitivity(app: &mut App) {
    let _ = app;
    todo!("A1")
}
