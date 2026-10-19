//! Port of app.tsx.
//!
//! The App component, split by concern: [`state`] holds its state, [`event_loop`] runs it, [`effects`] are its
//! `useEffect`s and daemon subscription, [`layout`] its geometry, header and footer, and [`modes`] each mode's keys and
//! screen. This module has what they share: the helpers at the top of app.tsx, the derived values (`sessions`,
//! `visibleSessions`, `selectedSession`, ...), the trust-review gate (`reviewThen`), the key dispatcher and the
//! [`UiModel`] / [`FlowHost`] glue.
//!
//! Calling a flow: build its props from the app's other fields and call it on `app.flows.<flow>` with a
//! [`crate::flow::FlowCx`] whose locator finds that flow, then apply the effects (destructure `App` so the flow and
//! the props' sources are disjoint borrows).

pub mod effects;
pub mod event_loop;
pub mod layout;
pub mod modes;
pub mod state;

use std::io;

use ratatui::Frame;
use ratatui::style::Color;

use deckhand_core::key::Input;
use deckhand_protocol::data::RightPaneTab;
use deckhand_protocol::data::{
    AttachTarget, BranchList, ProgramKey, ProjectInfo, SessionCleanupInspection, SessionRecord, SubSessionKind,
    UiExitResult, WorktreeMode,
};

use crate::app::event_loop::UiModel;
use crate::app::state::{
    App, AppAction, AppCallback, AppProps, CleanupCheck, Mode, ReviewLabels, ReviewSkip, WorkspaceInfo,
};
use crate::flow::{FlowEffect, FlowHost};
use crate::input::KeyEvent;

/// TS: app.tsx RIGHT_TABS (Tab / ] order).
pub const RIGHT_TABS: [RightPaneTab; 5] = [
    RightPaneTab::Preview,
    RightPaneTab::Terminal,
    RightPaneTab::Git,
    RightPaneTab::Dev,
    RightPaneTab::Notes,
];

/// TS: app.tsx PROGRAMS entries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProgramOption {
    pub key: ProgramKey,
    pub label: &'static str,
    pub glyph: &'static str,
}

/// TS: app.tsx PROGRAMS (the new-session picker's agents, in order).
pub const PROGRAMS: [ProgramOption; 3] = [
    ProgramOption {
        key: ProgramKey::Claude,
        label: "Claude",
        glyph: "✶",
    },
    ProgramOption {
        key: ProgramKey::Pi,
        label: "Pi",
        glyph: "π",
    },
    ProgramOption {
        key: ProgramKey::Codex,
        label: "Codex",
        glyph: "◇",
    },
];

/// TS: app.tsx SPINNER_FRAMES.
pub const SPINNER_FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// TS: app.tsx WORKTREE_MODES entries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WorktreeModeOption {
    pub key: WorktreeMode,
    pub label: &'static str,
}

/// TS: app.tsx WORKTREE_MODES (Tab in the new-session form cycles them).
pub const WORKTREE_MODES: [WorktreeModeOption; 3] = [
    WorktreeModeOption {
        key: WorktreeMode::None,
        label: "no worktree",
    },
    WorktreeModeOption {
        key: WorktreeMode::New,
        label: "new worktree",
    },
    WorktreeModeOption {
        key: WorktreeMode::Existing,
        label: "existing worktree",
    },
];

/// TS: app.tsx SCROLL_SENSITIVITY_STEP (+ / − in preview focus).
pub const SCROLL_SENSITIVITY_STEP: f64 = 0.04;

/// TS: app.tsx DEV_DOUBLE_TAP_MS: two d presses within this long start or stop Dev.
pub const DEV_DOUBLE_TAP_MS: i64 = 500;

/// TS: app.tsx STATUS_MESSAGE_AUTO_HIDE_MS.
pub const STATUS_MESSAGE_AUTO_HIDE_MS: u64 = 5000;

/// TS: app.tsx ERROR_MESSAGE_AUTO_HIDE_MS.
pub const ERROR_MESSAGE_AUTO_HIDE_MS: u64 = 8000;

/// TS: app.tsx normalizeScrollSensitivity (0..1, the default when unset or not finite).
pub fn normalize_scroll_sensitivity(value: Option<f64>) -> f64 {
    let _ = value;
    todo!("A1")
}

/// TS: app.tsx formatScrollSensitivity (`0.12`, `0.1`, `1`).
pub fn format_scroll_sensitivity(value: f64) -> String {
    let _ = value;
    todo!("A1")
}

/// Mouse wheel direction.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum WheelDirection {
    Up,
    Down,
}

/// TS: app.tsx mouseWheelSequence: `count` SGR wheel reports at the pane's center (sent to Claude to scroll it).
pub fn mouse_wheel_sequence(direction: WheelDirection, cols: u16, rows: u16, count: usize) -> String {
    let _ = (direction, cols, rows, count);
    todo!("A1")
}

/// TS: app.tsx parseMouseWheel's result.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MouseWheel {
    pub direction: WheelDirection,
    pub count: usize,
}

/// TS: app.tsx parseMouseWheel: the net wheel movement in a chunk of raw input (SGR and legacy reports).
pub fn parse_mouse_wheel(input: &str) -> Option<MouseWheel> {
    let _ = input;
    todo!("A1")
}

/// TS: app.tsx sanitizeNameInput: typed name text without controls, orphaned escape/mouse fragments or characters
/// outside `a-zA-Z0-9 _-/.:[]()#`.
pub fn sanitize_name_input(input: &str) -> String {
    let _ = input;
    todo!("A1")
}

/// TS: app.tsx sortSessions.
pub fn sort_sessions(sessions: Vec<SessionRecord>) -> Vec<SessionRecord> {
    let _ = sessions;
    todo!("A1")
}

/// TS: app.tsx upsertSession: `session` replacing its old record, then sorted.
pub fn upsert_session(existing: Vec<SessionRecord>, session: SessionRecord) -> Vec<SessionRecord> {
    let _ = (existing, session);
    todo!("A1")
}

/// TS: app.tsx describeConnection (`ready` / `connecting…`).
pub fn describe_connection(connected: bool) -> &'static str {
    let _ = connected;
    todo!("A1")
}

/// TS: app.tsx connectionColor.
pub fn connection_color(connected: bool) -> Color {
    let _ = connected;
    todo!("A1")
}

/// TS: app.tsx supportsForkedSubSession.
pub fn supports_forked_sub_session(session: Option<&SessionRecord>) -> bool {
    let _ = session;
    todo!("A1")
}

/// TS: app.tsx forkStaysInParent: forks of an agent that may reopen them in the parent's directory run there.
pub fn fork_stays_in_parent(program: Option<ProgramKey>, sub_session_kind: Option<SubSessionKind>) -> bool {
    let _ = (program, sub_session_kind);
    todo!("A1")
}

/// TS: app.tsx parentWorkspaceLabel (`parent worktree: …` / `parent dir: …`).
pub fn parent_workspace_label(session: Option<&SessionRecord>, width: usize) -> Option<String> {
    let _ = (session, width);
    todo!("A1")
}

/// TS: app.tsx sessionTitleFromTask: a session title (at most 64 characters) from a task's, cut at a word.
pub fn session_title_from_task(title: &str) -> String {
    let _ = title;
    todo!("A1")
}

/// TS: app.tsx baseOptions entries.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BaseOption {
    pub label: String,
    pub value: Option<String>,
}

/// TS: app.tsx baseOptions: where a new worktree's branch can start (the setting first, then local branches).
pub fn base_options(list: Option<&BranchList>) -> Vec<BaseOption> {
    let _ = list;
    todo!("A1")
}

/// A function of the reviewed project (TS reviewThen's `gate`, `labels`, `purpose`).
pub type ProjectFn<T> = std::rc::Rc<dyn Fn(&ProjectInfo) -> T>;

/// TS: app.tsx reviewThen's options.
#[derive(Clone, Default)]
pub struct ReviewOptions {
    /// Whether the action depends on an untrusted override (default: `project.needsReview`).
    pub gate: Option<ProjectFn<bool>>,
    /// The action does not run when the config cannot be read.
    pub required: bool,
    pub back: Option<AppCallback>,
    pub skip: ReviewSkip,
    pub labels: Option<ProjectFn<ReviewLabels>>,
    pub purpose: Option<ProjectFn<String>>,
}

/// What the scrollable details modes show (TS detailsContent).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DetailsContent {
    pub title: String,
    pub text: String,
    pub footer: Vec<deckhand_core::display::HintPart>,
    pub scroll: usize,
}

impl App {
    /// TS `sessions`: this checkout's sessions, replies read before the last detach marked read.
    pub fn sessions(&self) -> Vec<SessionRecord> {
        todo!("A1")
    }

    /// TS `collapseApplied`: search and non-default filters reveal matches regardless of collapse state.
    pub fn collapse_applied(&self) -> bool {
        todo!("A1")
    }

    /// TS `visibleSessions`: the filtered, searched and (when applied) collapsed list.
    pub fn visible_sessions(&self) -> Vec<SessionRecord> {
        todo!("A1")
    }

    /// TS `selectedIndex` (0 without a visible selection).
    pub fn selected_index(&self) -> usize {
        todo!("A1")
    }

    /// TS `selectedSession`: only a visible session is actionable.
    pub fn selected_session(&self) -> Option<SessionRecord> {
        todo!("A1")
    }

    /// TS `currentWorkspaceInfo`.
    pub fn current_workspace_info(&self) -> Option<&WorkspaceInfo> {
        todo!("A1")
    }

    /// TS `currentCleanupCheck`.
    pub fn current_cleanup_check(&self) -> Option<&CleanupCheck> {
        todo!("A1")
    }

    /// TS `cleanupInspectionFor(deleteBranch)`.
    pub fn cleanup_inspection_for(&self, delete_branch: bool) -> Option<&SessionCleanupInspection> {
        let _ = delete_branch;
        todo!("A1")
    }

    /// TS `showingAction`: the Terminal tab shows the workspace's last action.
    pub fn showing_action(&self) -> bool {
        todo!("A1")
    }

    /// TS `activeAttachTarget`.
    pub fn active_attach_target(&self) -> AttachTarget {
        todo!("A1")
    }

    /// TS `selectedWorkspace`.
    pub fn selected_workspace(&self) -> Option<String> {
        todo!("A1")
    }

    /// TS `runListHasDev`.
    pub fn run_list_has_dev(&self) -> bool {
        todo!("A1")
    }

    /// TS `activePaneReadyForAttach`.
    pub fn active_pane_ready_for_attach(&self) -> bool {
        todo!("A1")
    }

    /// TS `selectedCanDeleteWorktree`.
    pub fn selected_can_delete_worktree(&self) -> bool {
        todo!("A1")
    }

    /// TS `selectedCanDeleteBranch`.
    pub fn selected_can_delete_branch(&self) -> bool {
        todo!("A1")
    }

    /// TS `spinnerFrame`.
    pub fn spinner_frame(&self) -> &'static str {
        todo!("A1")
    }

    /// TS `setMode`.
    pub fn set_mode(&mut self, mode: Mode) {
        let _ = mode;
        todo!("A1")
    }

    /// TS `setError` (auto-hidden after ERROR_MESSAGE_AUTO_HIDE_MS).
    pub fn set_error(&mut self, error: Option<String>) {
        let _ = error;
        todo!("A1")
    }

    /// TS `setStatusMessage` (auto-hidden after STATUS_MESSAGE_AUTO_HIDE_MS).
    pub fn set_status(&mut self, status: Option<String>) {
        let _ = status;
        todo!("A1")
    }

    /// TS `setBusy`.
    pub fn set_busy(&mut self, busy: bool) {
        let _ = busy;
        todo!("A1")
    }

    /// TS `setSelectedId` (the selected-id effect: reset the scroll, restore the session's tab, report it).
    pub fn set_selected_id(&mut self, id: Option<String>) {
        let _ = id;
        todo!("A1")
    }

    /// TS `setActiveTab` (the active-tab effect: remember it for the session, leave tab-bound focus modes).
    pub fn set_active_tab(&mut self, tab: RightPaneTab) {
        let _ = tab;
        todo!("A1")
    }

    /// TS `exit(result)` (useApp): the loop ends with it.
    pub fn exit_with(&mut self, result: UiExitResult) {
        let _ = result;
        todo!("A1")
    }

    /// TS `quit` (q, Ctrl+C).
    pub fn quit(&mut self) {
        todo!("A1")
    }

    /// TS `attachTo`: exit to the attach hand-off for `session`'s `target`.
    pub fn attach_to(&mut self, session: &SessionRecord, target: AttachTarget) {
        let _ = (session, target);
        todo!("A1")
    }

    /// TS `elsewhere`: a session of another checkout is in the sidebar of the Deckhand opened there (says so).
    pub fn elsewhere(&mut self, session_id: &str) -> bool {
        let _ = session_id;
        todo!("A1")
    }

    /// TS `revealSession`: clears the search, widens a filter that hides it and expands its collapsed ancestors.
    pub fn reveal_session(&mut self, session_id: &str) {
        let _ = session_id;
        todo!("A1")
    }

    /// TS `goToSession` (o on a task, enter on a worktree): reveal and select it, back to browse.
    pub fn go_to_session(&mut self, session_id: &str) {
        let _ = session_id;
        todo!("A1")
    }

    /// TS `editNoteInTerminal` (o on Notes): exit to `$EDITOR` on the active note.
    pub fn edit_note_in_terminal(&mut self, session: &SessionRecord) {
        let _ = session;
        todo!("A1")
    }

    /// TS `reviewThen`: resolves the repository config for `target_cwd` and runs `resume` with it, showing the
    /// inline review first when `gate` says the action depends on an untrusted override.
    pub fn review_then(
        &mut self,
        target_cwd: &str,
        resume: Option<AppAction<Option<ProjectInfo>>>,
        options: ReviewOptions,
    ) {
        let _ = (target_cwd, resume, options);
        todo!("A1")
    }

    /// TS `detailsContent`: title, text and footer of the scrollable details modes.
    pub fn details_content(&self) -> Option<DetailsContent> {
        todo!("A1")
    }

    /// TS `scrollDetailsPane`: shared scrolling of the details modes; true when the key scrolled.
    pub fn scroll_details_pane(&mut self, input: &Input) -> bool {
        let _ = input;
        todo!("A1")
    }

    /// TS: the useTerminalInput handler: Ctrl+C, the busy gate, then the mode's keys (crates/ui/src/app/modes).
    pub fn handle_input(&mut self, input: &Input) {
        let _ = input;
        todo!("A1")
    }
}

impl FlowHost for App {
    /// TS: the flows' callbacks (onExit → browse, setMode, setError, onGoTo → goToSession, ...).
    fn apply_flow_effects(&mut self, effects: Vec<FlowEffect<Self>>) {
        let _ = effects;
        todo!("A1")
    }
}

impl UiModel for App {
    type Exit = UiExitResult;

    /// TS: the preview-focus `process.stdin.on('data')` listener (mouse wheel).
    fn on_stdin_text(&mut self, text: &str) {
        let _ = text;
        todo!("A1")
    }

    /// TS: useTerminalInput (releases dropped, the key normalised) → [`App::handle_input`].
    fn on_key(&mut self, event: &KeyEvent) {
        let _ = event;
        todo!("A1")
    }

    /// TS: the resize effect.
    fn on_resize(&mut self, cols: u16, rows: u16) {
        let _ = (cols, rows);
        todo!("A1")
    }

    /// TS: every `useEffect` of App (crates/ui/src/app/effects.rs).
    fn after_update(&mut self) {
        todo!("A1")
    }

    /// TS: App's render (crates/ui/src/app/layout.rs).
    fn render(&mut self, frame: &mut Frame<'_>) {
        let _ = frame;
        todo!("A1")
    }

    fn exit(&mut self) -> Option<UiExitResult> {
        todo!("A1")
    }
}

/// TS: cli.ts runUi's Ink part: renders App on the terminal (raw stdin, resize, the event loop) until it exits.
/// The caller (crates/deckhand/src/ui_loop.rs) owns the alternate screen, the UI state file and what follows the
/// exit (attach, `$EDITOR`).
pub async fn run(props: AppProps) -> io::Result<Option<UiExitResult>> {
    let _ = props;
    todo!("A1")
}
