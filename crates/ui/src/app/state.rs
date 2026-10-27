//! State: the `App` component's props, `useState`s and `useRef`s, grouped by the mode that owns their behaviour so each
//! mode unit (crates/ui/src/app/modes) works on its own group.
//!
//! Field names follow the TS state names; initial values are the TS `useState` initialisers ([`App::new`]).
//! Derived values (`sessions`, `visibleSessions`, `selectedSession`, `layout`, ...) are computed on demand by the
//! app's methods, not stored. Closures TS keeps in state (the trust review's `resume`/`skip`/`back`) are
//! [`AppAction`]s / [`AppCallback`]s run on the app.

use std::rc::Rc;

use indexmap::{IndexMap, IndexSet};

use deckhand_client::live::LiveClient;
use deckhand_core::display::HintPart;
use deckhand_protocol::data::{
    ActionRecord, AgentVersions, BranchList, ChangesRecord, DevRecord, MergePreview, PreviewRecord, ProjectInfo,
    RightPaneTab, SessionCleanupInspection, SessionRecord, SubSessionKind, Task, TasksDoc, TerminalRecord,
    UiExitResult, WorkspaceSummary, WorktreeInfoRecord, WorktreeMergeResult, WorktreeMode,
};
use deckhand_protocol::records::SessionFilter;
use deckhand_sys::async_util::{Interval, Task as LocalTask, Timer};

use crate::app::effects::EffectState;
use crate::app::event_loop::Dispatch;
use crate::flows::agents::AgentsFlow;
use crate::flows::changes::ChangesFlow;
use crate::flows::notes::NotesFlow;
use crate::flows::settings::SettingsFlow;
use crate::flows::tasks::TasksFlow;
use crate::flows::worktrees::WorktreesFlow;
use crate::panes::help::HelpView;
use crate::panes::terminal::TerminalView;

/// TS: app.tsx Mode: which screen has the keys (and the right pane).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Mode {
    #[default]
    Browse,
    PreviewFocus,
    ChangesFocus,
    NotesFocus,
    PickProgram,
    EnterName,
    PickWorktree,
    ConfirmKill,
    ConfirmMerge,
    MergeConflicts,
    Help,
    Settings,
    EditProject,
    DiscardProject,
    Search,
    WorkspaceInfo,
    ReviewProject,
    PickAction,
    ConfirmLoss,
    Agents,
    Tasks,
    ConfirmRemove,
    PickFilter,
    Worktrees,
}

impl Mode {
    /// Every mode, in the TS union's order.
    pub const ALL: [Mode; 24] = [
        Mode::Browse,
        Mode::PreviewFocus,
        Mode::ChangesFocus,
        Mode::NotesFocus,
        Mode::PickProgram,
        Mode::EnterName,
        Mode::PickWorktree,
        Mode::ConfirmKill,
        Mode::ConfirmMerge,
        Mode::MergeConflicts,
        Mode::Help,
        Mode::Settings,
        Mode::EditProject,
        Mode::DiscardProject,
        Mode::Search,
        Mode::WorkspaceInfo,
        Mode::ReviewProject,
        Mode::PickAction,
        Mode::ConfirmLoss,
        Mode::Agents,
        Mode::Tasks,
        Mode::ConfirmRemove,
        Mode::PickFilter,
        Mode::Worktrees,
    ];

    /// The TS name (`'preview-focus'`, ...).
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Browse => "browse",
            Mode::PreviewFocus => "preview-focus",
            Mode::ChangesFocus => "changes-focus",
            Mode::NotesFocus => "notes-focus",
            Mode::PickProgram => "pick-program",
            Mode::EnterName => "enter-name",
            Mode::PickWorktree => "pick-worktree",
            Mode::ConfirmKill => "confirm-kill",
            Mode::ConfirmMerge => "confirm-merge",
            Mode::MergeConflicts => "merge-conflicts",
            Mode::Help => "help",
            Mode::Settings => "settings",
            Mode::EditProject => "edit-project",
            Mode::DiscardProject => "discard-project",
            Mode::Search => "search",
            Mode::WorkspaceInfo => "workspace-info",
            Mode::ReviewProject => "review-project",
            Mode::PickAction => "pick-action",
            Mode::ConfirmLoss => "confirm-loss",
            Mode::Agents => "agents",
            Mode::Tasks => "tasks",
            Mode::ConfirmRemove => "confirm-remove",
            Mode::PickFilter => "pick-filter",
            Mode::Worktrees => "worktrees",
        }
    }

    /// The mode with this TS name.
    pub fn from_name(name: &str) -> Option<Mode> {
        Mode::ALL.into_iter().find(|mode| mode.as_str() == name)
    }

    /// TS: the modes that keep the session list and the tabbed right pane on screen.
    pub fn shows_tabs(self) -> bool {
        matches!(
            self,
            Mode::Browse | Mode::PreviewFocus | Mode::ChangesFocus | Mode::NotesFocus | Mode::Search | Mode::PickFilter
        )
    }

    /// Not in the TS: a mode drawn as a dialog over the (faded) tabbed pane rather than in its place.
    pub fn is_dialog(self) -> bool {
        matches!(
            self,
            Mode::PickProgram
                | Mode::EnterName
                | Mode::PickWorktree
                | Mode::PickAction
                | Mode::ConfirmKill
                | Mode::ConfirmRemove
        )
    }

    /// Not in the TS: a dialog, or a full-pane confirmation (merge and its conflicts), whose body scrolls with
    /// J/K (PgUp/PgDn).
    pub fn scrolls_body(self) -> bool {
        self.is_dialog() || matches!(self, Mode::ConfirmMerge | Mode::MergeConflicts)
    }
}

/// TS: app.tsx TerminalSize (`process.stdout.columns || 80`, `rows || 24`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TerminalSize {
    pub cols: u16,
    pub rows: u16,
}

impl Default for TerminalSize {
    fn default() -> Self {
        TerminalSize { cols: 80, rows: 24 }
    }
}

/// A deferred action on the app taking a value (TS closures kept in state, e.g. the review's `resume`).
pub type AppAction<T> = Rc<dyn Fn(&mut App, T)>;
/// A deferred action on the app (TS `() => void` kept in state, e.g. the review's `back`).
pub type AppCallback = Rc<dyn Fn(&mut App)>;

/// A prop callback reporting one value (TS `(value) => void`).
pub type Listener<T> = Box<dyn Fn(T)>;
/// TS `onSelectedIdChange`.
pub type SelectedIdListener = Box<dyn Fn(Option<&str>)>;
/// TS `onSessionTabChange`.
pub type SessionTabListener = Box<dyn Fn(&str, RightPaneTab)>;
/// TS `onCollapsedSessionIdsChange` / `onHiddenExitedSessionIdsChange`.
pub type IdsListener = Box<dyn Fn(&[String])>;
/// TS `onSessionVisibilityChange`.
pub type VisibilityListener = Box<dyn Fn(SessionFilter, &str)>;

/// TS: app.tsx AppProps callbacks (cli.ts persists the UI state through them).
#[derive(Default)]
pub struct AppCallbacks {
    pub on_terminal_view_change: Option<Listener<TerminalView>>,
    pub on_selected_id_change: Option<SelectedIdListener>,
    pub on_active_tab_change: Option<Listener<RightPaneTab>>,
    pub on_session_tab_change: Option<SessionTabListener>,
    pub on_collapsed_session_ids_change: Option<IdsListener>,
    pub on_hidden_exited_session_ids_change: Option<IdsListener>,
    pub on_session_visibility_change: Option<VisibilityListener>,
    /// Not in the TS: the tasks whose steps the Tasks view hides.
    pub on_collapsed_task_steps_change: Option<IdsListener>,
}

impl std::fmt::Debug for AppCallbacks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppCallbacks").finish_non_exhaustive()
    }
}

/// TS: app.tsx AppProps.
#[derive(Debug, Default)]
pub struct AppProps {
    pub repo_root: String,
    pub cwd: String,
    pub initial_selected_id: Option<String>,
    pub initial_active_tab: Option<RightPaneTab>,
    pub initial_session_tabs: Option<IndexMap<String, RightPaneTab>>,
    pub initial_collapsed_session_ids: Option<Vec<String>>,
    pub initial_hidden_exited_session_ids: Option<Vec<String>>,
    pub initial_session_filter: Option<SessionFilter>,
    pub initial_session_query: Option<String>,
    /// The Terminal tab's view, kept across attaches by the caller.
    pub initial_terminal_view: Option<TerminalView>,
    /// Not in the TS: the selected Dev process (by name), kept across attaches by the caller.
    pub initial_dev_process: Option<String>,
    /// Not in the TS: the tasks whose steps the Tasks view hides (c), kept across attaches by the caller.
    pub initial_collapsed_task_steps: Option<Vec<String>>,
    pub callbacks: AppCallbacks,
}

/// The daemon connection and the loop plumbing (TS `client`, `connectionEpoch`, `useApp().exit`).
#[derive(Debug)]
pub struct Connection {
    pub dispatch: Dispatch<App>,
    pub client: Option<Rc<LiveClient>>,
    /// Bumped to reconnect (TS `connectionEpoch`).
    pub epoch: u64,
    /// The connect attempt in flight.
    pub connecting: Option<LocalTask<()>>,
    pub reconnect_timer: Option<Timer>,
    /// TS `exit(result)`: what the loop ends with.
    pub exit: Option<UiExitResult>,
}

/// The session list, the selection and the tabs: browse's state.
#[derive(Debug)]
pub struct BrowseState {
    pub session_filter: SessionFilter,
    pub session_query: String,
    pub collapsed_session_ids: IndexSet<String>,
    pub hidden_exited_session_ids: IndexSet<String>,
    pub selected_id: Option<String>,
    /// TS `sessionTabsRef`: each session's last tab.
    pub session_tabs: IndexMap<String, RightPaneTab>,
    pub active_tab: RightPaneTab,
    /// Digits typed to select a session by number (10+ sessions).
    pub numeric_selection: String,
    /// TS `lastDevTapRef`: when d was last pressed (ms since the epoch; 0 for none).
    pub last_dev_tap: i64,
    pub spinner_index: usize,
    /// Re-render tick when the selected session's shown age changes.
    pub clock_tick: u64,
    pub terminal_size: TerminalSize,
    /// Not in the TS: the share of the terminal you chose for the sidebar, in percent (`sidebar_width_percent`, which
    /// `<`/`>` set and `=` clears; the TS saved a width per repository). `None`: fitted to the sessions.
    pub sidebar_percent: Option<f64>,
    /// Not in the TS: the widest listed row's whole width, measured once they load and after each resize: the
    /// sidebar shrinks to it when every row fits in less than its share.
    pub sidebar_content: Option<i64>,
    /// Not in the TS: the rows are measured again on the next pass (set at launch and by a resize).
    pub sidebar_fit_pending: bool,
}

/// What the right pane's tabs show (the daemon's watched records).
#[derive(Debug)]
pub struct PaneRecords {
    pub preview: PreviewRecord,
    pub terminal: TerminalRecord,
    pub changes: ChangesRecord,
    /// TS `changesWatchedRef`: this UI asked the daemon to watch a workspace's changes.
    pub changes_watched: bool,
    pub dev: DevRecord,
    /// Not in the TS: how many `dev-updated` events have replaced `dev`. A start reply is a snapshot taken when Dev
    /// started; an update counted after its request went out is newer (a quick command may already have exited).
    pub dev_updates: u64,
    /// Not in the TS: the Dev process selected in the split (v switches, enter attaches it), by name.
    pub dev_process: Option<String>,
    /// The workspace's last action (Terminal tab, v).
    pub action: ActionRecord,
    pub terminal_view: TerminalView,
}

/// The repository's data from the daemon.
#[derive(Debug, Default)]
pub struct RepoData {
    /// Every session of the repository (the daemon sends all its checkouts'); the sidebar shows the ones of this
    /// checkout (TS `sessions`, derived).
    pub repo_sessions: Vec<SessionRecord>,
    pub sessions_loaded: bool,
    pub agent_versions: Option<AgentVersions>,
    pub tasks_doc: Option<TasksDoc>,
    /// TS `tasks`: `parseTasks(tasksDoc?.text ?? '')`, reparsed when the text changes.
    pub tasks: Vec<Task>,
}

/// The footer's messages and the busy gate.
#[derive(Debug, Default)]
pub struct Messages {
    pub error: Option<String>,
    pub status_message: Option<String>,
    /// The status message when it is a notice ([`crate::app::App::show_notice`]): the next key hides it too.
    pub notice: Option<String>,
    /// While set, keys other than Ctrl+C are ignored.
    pub busy: bool,
    pub error_timer: Option<Timer>,
    pub status_timer: Option<Timer>,
}

/// Preview focus (→ on a running session's Preview): scrolling and the mouse wheel.
#[derive(Debug)]
pub struct PreviewFocusState {
    pub scroll_offset: i64,
    pub scroll_sensitivity: f64,
    /// TS `previewWheelAccumulatorRef`.
    pub wheel: deckhand_core::wheel::WheelScaler,
}

/// TS: the filter menu's way back (f): the filter and selection it was opened on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FilterMenuBack {
    pub filter: SessionFilter,
    pub selected_id: Option<String>,
}

/// TS: app.tsx ReviewLabels (the gated step's Enter and s hints).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReviewLabels {
    pub enter: Option<HintPart>,
    pub skip: Option<HintPart>,
}

/// What s does on the trust review (TS `skip`: `undefined` resumes ignoring the override, `null` cancels).
#[derive(Clone, Default)]
pub enum ReviewSkip {
    #[default]
    Resume,
    Cancel,
    Run(AppAction<ProjectInfo>),
}

impl std::fmt::Debug for ReviewSkip {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            ReviewSkip::Resume => "Resume",
            ReviewSkip::Cancel => "Cancel",
            ReviewSkip::Run(_) => "Run(..)",
        })
    }
}

/// TS: app.tsx `review` state: the inline repository-config review, the cwd it was resolved for and the action it
/// gates (none for an explicit T review).
#[derive(Clone)]
pub struct Review {
    pub project: ProjectInfo,
    pub cwd: String,
    pub resume: Option<AppAction<Option<ProjectInfo>>>,
    pub skip: ReviewSkip,
    pub labels: Option<ReviewLabels>,
    pub purpose: Option<String>,
    /// Where closing the review returns (Settings' T); otherwise browse.
    pub back: Option<AppCallback>,
}

impl std::fmt::Debug for Review {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Review")
            .field("cwd", &self.cwd)
            .field("skip", &self.skip)
            .field("labels", &self.labels)
            .field("purpose", &self.purpose)
            .finish_non_exhaustive()
    }
}

/// review-project mode, and the scroll shared by the details modes (review, workspace info, confirm-loss).
#[derive(Debug, Default)]
pub struct ReviewState {
    pub review: Option<Review>,
    pub details_scroll: usize,
}

/// TS: app.tsx `actionProject`: the project (and session) the run list was opened for.
#[derive(Clone, Debug, PartialEq)]
pub struct ActionProject {
    pub project: ProjectInfo,
    pub cwd: String,
    pub session_id: Option<String>,
}

/// pick-action mode (r, the run list).
#[derive(Debug, Default)]
pub struct ActionPickerState {
    pub action_project: Option<ActionProject>,
    pub action_index: usize,
}

/// TS: app.tsx `workspaceInfo`: keyed by session so a late response never shows in another.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WorkspaceInfo {
    pub session_id: String,
    pub summary: Option<WorkspaceSummary>,
    pub pr_loading: bool,
    pub confirm_pr: bool,
    pub creating_pr: bool,
}

/// workspace-info mode (i).
#[derive(Debug, Default)]
pub struct WorkspaceInfoState {
    pub info: Option<WorkspaceInfo>,
    /// TS `workspaceRequestRef`.
    pub request: u64,
}

/// TS: app.tsx `cleanupCheck`: the deletion inspections of one session (worktree alone, and with its branch).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CleanupCheck {
    pub session_id: String,
    pub worktree: Option<SessionCleanupInspection>,
    pub branch: Option<SessionCleanupInspection>,
}

/// confirm-kill and confirm-loss modes (x, and typing DELETE).
#[derive(Debug, Default)]
pub struct KillState {
    pub cleanup_check: Option<CleanupCheck>,
    /// TS `cleanupRequestRef`.
    pub cleanup_request: u64,
    pub confirm_index: usize,
    pub force: bool,
    /// The worktree note's open items stay in it (space) instead of going to the backlog.
    pub keep_open_items: bool,
    /// confirm-loss: what was typed (DELETE).
    pub confirmation_draft: String,
    pub pending_delete_branch: bool,
    /// TS: the 1.5 s "still running; press X" check after a kill.
    pub still_running_timer: Option<Timer>,
}

/// TS: app.tsx MergeFlow: the merge confirmation (m) of one session.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MergeFlow {
    pub session_id: String,
    pub preview: Option<MergePreview>,
    pub preview_error: Option<String>,
    pub commit_first: bool,
    pub error: Option<String>,
    /// The selection is on the note's open items row (else on the options).
    pub on_items: bool,
    /// The note's open items stay in the note.
    pub keep_open_items: bool,
}

/// TS: app.tsx `mergeConflict`: a conflicted merge's result, until it is kept or aborted.
#[derive(Clone, Debug, PartialEq)]
pub struct MergeConflict {
    pub session_id: String,
    pub result: WorktreeMergeResult,
}

/// confirm-merge and merge-conflicts modes (m).
#[derive(Debug, Default)]
pub struct MergeState {
    pub confirm_index: usize,
    pub flow: Option<MergeFlow>,
    /// TS `mergeRequestRef`.
    pub request: u64,
    pub conflict: Option<MergeConflict>,
}

/// confirm-remove mode (Backspace on an archived session).
#[derive(Debug, Default)]
pub struct RemoveState {
    /// Open checklist items of the notes removing it deletes.
    pub items: Vec<String>,
}

/// TS: app.tsx `taskStart`: the task a new session starts from (n on the board).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskStart {
    pub id: String,
    pub title: String,
}

/// pick-program, enter-name and pick-worktree modes (n / N).
#[derive(Debug)]
pub struct CreateState {
    pub program_index: usize,
    pub draft_name: String,
    pub parent_id: Option<String>,
    pub sub_session_kind: Option<SubSessionKind>,
    pub worktree_mode: WorktreeMode,
    pub worktrees: Vec<WorktreeInfoRecord>,
    pub worktree_query: String,
    /// Typing into the existing-worktree search (after /).
    pub worktree_typing: bool,
    pub worktree_index: usize,
    /// A clean child starting from the parent's exported handoff.
    pub handoff_from_id: Option<String>,
    pub task_start: Option<TaskStart>,
    pub branch_list: Option<BranchList>,
    /// The new worktree's base (↑↓; 0: the setting).
    pub base_index: usize,
}

/// The *Flow.tsx screens and the help guide (TS: their hooks' state).
#[derive(Default)]
pub struct Flows {
    pub help: HelpView,
    pub agents: AgentsFlow,
    pub changes: ChangesFlow,
    pub notes: NotesFlow,
    pub settings: SettingsFlow,
    pub tasks: TasksFlow,
    pub worktrees: WorktreesFlow,
}

impl std::fmt::Debug for Flows {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Flows").finish_non_exhaustive()
    }
}

/// Timers and intervals the app runs (each cancelled when replaced or dropped, like an effect's cleanup).
#[derive(Debug, Default)]
pub struct AppTimers {
    /// The status spinner while a session works (120 ms).
    pub spinner: Option<Interval>,
    /// Confirms a typed session number after 300 ms.
    pub numeric_selection: Option<Timer>,
    /// Re-renders when the selected session's shown age changes.
    pub clock: Option<Timer>,
}

/// Not in the TS: how far the open dialog's body is scrolled, and how it scrolled when last drawn.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DialogState {
    /// The dialog mode the scroll belongs to; another dialog starts at the top.
    pub mode: Option<Mode>,
    pub scroll: usize,
    pub last: crate::primitives::DialogScroll,
}

/// TS: the App component: every piece of its state.
#[derive(Debug)]
pub struct App {
    pub props: AppProps,
    pub connection: Connection,
    pub mode: Mode,
    pub browse: BrowseState,
    pub panes: PaneRecords,
    pub data: RepoData,
    pub messages: Messages,
    pub preview_focus: PreviewFocusState,
    pub filter: Option<FilterMenuBack>,
    pub review: ReviewState,
    pub action_picker: ActionPickerState,
    pub workspace_info: WorkspaceInfoState,
    pub kill: KillState,
    pub merge: MergeState,
    pub remove: RemoveState,
    pub create: CreateState,
    /// Not in the TS: the open dialog's body scroll (J/K, PgUp/PgDn).
    pub dialog: DialogState,
    pub flows: Flows,
    pub timers: AppTimers,
    /// The effects' last dependencies and their in-flight work (crates/ui/src/app/effects.rs).
    pub effects: EffectState,
}

/// TS: app.tsx DEFAULT_SCROLL_SENSITIVITY (now 1: every wheel event).
pub use deckhand_core::wheel::DEFAULT_SCROLL_SENSITIVITY;

/// TS: app.tsx EMPTY_PREVIEW.
pub fn empty_preview() -> PreviewRecord {
    PreviewRecord::default()
}

/// TS: app.tsx EMPTY_TERMINAL / EMPTY_DEV / EMPTY_ACTION (`{content: '', live: false}`).
pub fn empty_pane() -> TerminalRecord {
    TerminalRecord::default()
}

impl App {
    /// TS: the App component's initial state from its props (`useState` initialisers).
    pub fn new(props: AppProps, dispatch: Dispatch<App>, terminal_size: TerminalSize) -> App {
        let mut session_tabs = props.initial_session_tabs.clone().unwrap_or_default();
        if let (Some(id), Some(tab)) = (&props.initial_selected_id, props.initial_active_tab) {
            session_tabs.insert(id.clone(), tab);
        }
        let active_tab = match &props.initial_selected_id {
            Some(id) => session_tabs.get(id).copied().unwrap_or(RightPaneTab::Preview),
            None => props.initial_active_tab.unwrap_or(RightPaneTab::Preview),
        };
        App {
            connection: Connection {
                dispatch,
                client: None,
                epoch: 0,
                connecting: None,
                reconnect_timer: None,
                exit: None,
            },
            mode: Mode::Browse,
            browse: BrowseState {
                session_filter: props.initial_session_filter.unwrap_or(SessionFilter::Active),
                session_query: props.initial_session_query.clone().unwrap_or_default(),
                collapsed_session_ids: props.initial_collapsed_session_ids.iter().flatten().cloned().collect(),
                hidden_exited_session_ids: props
                    .initial_hidden_exited_session_ids
                    .iter()
                    .flatten()
                    .cloned()
                    .collect(),
                selected_id: props.initial_selected_id.clone(),
                session_tabs,
                active_tab,
                numeric_selection: String::new(),
                last_dev_tap: 0,
                spinner_index: 0,
                clock_tick: 0,
                terminal_size,
                sidebar_percent: None,
                sidebar_content: None,
                sidebar_fit_pending: true,
            },
            panes: PaneRecords {
                preview: empty_preview(),
                terminal: empty_pane(),
                changes: deckhand_core::changes_model::empty_changes(),
                changes_watched: false,
                dev: empty_pane(),
                dev_updates: 0,
                dev_process: props.initial_dev_process.clone(),
                action: empty_pane(),
                terminal_view: props.initial_terminal_view.unwrap_or_default(),
            },
            data: RepoData::default(),
            messages: Messages::default(),
            preview_focus: PreviewFocusState {
                scroll_offset: 0,
                scroll_sensitivity: DEFAULT_SCROLL_SENSITIVITY,
                wheel: deckhand_core::wheel::WheelScaler::default(),
            },
            filter: None,
            review: ReviewState::default(),
            action_picker: ActionPickerState::default(),
            workspace_info: WorkspaceInfoState::default(),
            kill: KillState::default(),
            merge: MergeState::default(),
            remove: RemoveState::default(),
            dialog: DialogState::default(),
            create: CreateState {
                program_index: 0,
                draft_name: String::new(),
                parent_id: None,
                sub_session_kind: None,
                worktree_mode: WorktreeMode::None,
                worktrees: Vec::new(),
                worktree_query: String::new(),
                worktree_typing: false,
                worktree_index: 0,
                handoff_from_id: None,
                task_start: None,
                branch_list: None,
                base_index: 0,
            },
            flows: Flows {
                tasks: TasksFlow {
                    collapsed_steps: props.initial_collapsed_task_steps.clone().unwrap_or_default(),
                    ..TasksFlow::default()
                },
                ..Flows::default()
            },
            timers: AppTimers::default(),
            effects: EffectState::default(),
            props,
        }
    }

    /// The dispatcher for updates from async work.
    pub fn dispatch(&self) -> &Dispatch<App> {
        &self.connection.dispatch
    }

    /// The daemon client, once connected.
    pub fn client(&self) -> Option<Rc<LiveClient>> {
        self.connection.client.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_names_round_trip_in_ts_order() {
        let names: Vec<&str> = Mode::ALL.iter().map(|mode| mode.as_str()).collect();
        assert_eq!(
            names,
            [
                "browse",
                "preview-focus",
                "changes-focus",
                "notes-focus",
                "pick-program",
                "enter-name",
                "pick-worktree",
                "confirm-kill",
                "confirm-merge",
                "merge-conflicts",
                "help",
                "settings",
                "edit-project",
                "discard-project",
                "search",
                "workspace-info",
                "review-project",
                "pick-action",
                "confirm-loss",
                "agents",
                "tasks",
                "confirm-remove",
                "pick-filter",
                "worktrees"
            ]
        );
        for mode in Mode::ALL {
            assert_eq!(Mode::from_name(mode.as_str()), Some(mode));
        }
        assert_eq!(Mode::from_name("nope"), None);
        assert!(Mode::Search.shows_tabs() && !Mode::Tasks.shows_tabs());
    }

    #[test]
    fn empty_records_match_ts() {
        let preview = empty_preview();
        assert_eq!(preview.content, "");
        assert!(!preview.live && preview.session_id.is_none());
        let pane = empty_pane();
        assert_eq!(pane.content, "");
        assert!(!pane.live);
        assert_eq!(TerminalSize::default(), TerminalSize { cols: 80, rows: 24 });
    }
}
