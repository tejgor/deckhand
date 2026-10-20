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
//! the props' sources are disjoint borrows). The `App::*_flow_call` helpers below do exactly that.

pub mod effects;
pub mod event_loop;
pub mod layout;
pub mod modes;
pub mod state;

use std::cell::RefCell;
use std::collections::HashSet;
use std::io;
use std::rc::Rc;
use std::sync::LazyLock;

use indexmap::IndexMap;
use ratatui::Frame;
use ratatui::style::Color;
use regex::Regex;

use deckhand_core::agents::agent_spec;
use deckhand_core::detail_texts::{
    WorkspaceSummaryOptions, cleanup_override_text, create_pr_confirm_text, trust_review_text, workspace_summary_text,
};
use deckhand_core::display::{HintPart, compact_path, display_session_title, strip_terminal_controls, truncate};
use deckhand_core::jsstr::js_len;
use deckhand_core::key::{Input, Key};
use deckhand_core::session_features::{TaskTitleOf, filter_session_list, with_reply_read};
use deckhand_core::session_order::{filter_collapsed_sessions, sort_sessions_for_sidebar};
use deckhand_core::session_scope::session_matches_scope;
use deckhand_core::tasks::parse_tasks;
use deckhand_core::tasks_board::{OwnedTaskLinkLookup, linked_tasks, task_link_lookup};
use deckhand_core::workspace::workspace_key;
use deckhand_protocol::data::RightPaneTab;
use deckhand_protocol::data::{
    AttachTarget, BranchFrom, BranchList, ProgramKey, ProjectInfo, SessionCleanupInspection, SessionRecord,
    SessionStatus, SessionWorktreeMode, SubSessionKind, TasksDoc, UiExitResult, WorktreeMode,
};
use deckhand_protocol::records::SessionFilter;
use deckhand_sys::desktop::resolve_terminal_editor;

use crate::app::event_loop::UiModel;
use crate::app::layout::{AppLayout, app_layout};
use crate::app::state::{
    App, AppAction, AppCallback, AppProps, CleanupCheck, Mode, Review, ReviewLabels, ReviewSkip, WorkspaceInfo,
    empty_pane, empty_preview,
};
use crate::flow::{Flow, FlowCx, FlowEffect, FlowHost};
use crate::flows::agents::{AgentsFlow, AgentsFlowProps};
use crate::flows::changes::{ChangesFlow, ChangesFlowProps};
use crate::flows::notes::{FileThen, NotesFlow, NotesFlowProps};
use crate::flows::settings::{SettingsFlow, SettingsFlowProps, is_settings_flow_mode};
use crate::flows::tasks::{TasksFlow, TasksFlowProps};
use crate::flows::worktrees::{WorktreesFlow, WorktreesFlowProps};
use crate::input::{KeyEvent, terminal_input};
use crate::panes::details::{details_viewport, scroll_details};
use crate::panes::help::HelpOutcome;
use crate::panes::terminal::{TerminalView, has_action};
use crate::theme::THEME;

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

/// TS: a string is truthy (`undefined`, `null` and `''` are not).
pub(crate) fn truthy(value: Option<&String>) -> bool {
    value.is_some_and(|text| !text.is_empty())
}

/// TS: app.tsx normalizeScrollSensitivity (0..1, the default when unset or not finite).
pub fn normalize_scroll_sensitivity(value: Option<f64>) -> f64 {
    match value {
        Some(value) if value.is_finite() => value.clamp(0.0, 1.0),
        _ => crate::app::state::DEFAULT_SCROLL_SENSITIVITY,
    }
}

/// JS `Number.prototype.toFixed(2)`: the exact decimal value rounded half up (on the magnitude) to two places.
fn js_to_fixed_2(value: f64) -> String {
    if value.is_nan() {
        return "NaN".to_owned();
    }
    if value.is_infinite() {
        return if value > 0.0 { "Infinity" } else { "-Infinity" }.to_owned();
    }
    if value.abs() >= 1e21 {
        // JS: `String(value)` from 1e21 on.
        return format!("{value:e}").replacen('e', "e+", 1);
    }
    // Every finite double has a finite decimal expansion; 1100 places hold all of it.
    let exact = format!("{:.1100}", value.abs());
    let (whole, fraction) = exact.split_once('.').unwrap_or((&exact, ""));
    let digits: Vec<u8> = fraction.bytes().map(|byte| byte - b'0').collect();
    let mut cents: u128 = whole.parse::<u128>().unwrap_or(0) * 100
        + u128::from(digits.first().copied().unwrap_or(0)) * 10
        + u128::from(digits.get(1).copied().unwrap_or(0));
    if digits.get(2).copied().unwrap_or(0) >= 5 {
        cents += 1;
    }
    let sign = if value < 0.0 && cents != 0 { "-" } else { "" };
    format!("{sign}{}.{:02}", cents / 100, cents % 100)
}

/// TS: app.tsx formatScrollSensitivity (`0.12`, `0.1`, `1`).
pub fn format_scroll_sensitivity(value: f64) -> String {
    let fixed = js_to_fixed_2(value);
    let trimmed = fixed.trim_end_matches('0');
    trimmed.strip_suffix('.').unwrap_or(trimmed).to_owned()
}

/// Mouse wheel direction.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum WheelDirection {
    Up,
    Down,
}

/// TS: app.tsx mouseWheelSequence: `count` SGR wheel reports at the pane's center (sent to Claude to scroll it).
pub fn mouse_wheel_sequence(direction: WheelDirection, cols: u16, rows: u16, count: usize) -> String {
    let button = match direction {
        WheelDirection::Up => 64,
        WheelDirection::Down => 65,
    };
    let x = (cols / 2).max(1);
    let y = (rows / 2).max(1);
    format!("\u{1b}[<{button};{x};{y}M").repeat(count.max(1))
}

/// TS: app.tsx parseMouseWheel's result.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MouseWheel {
    pub direction: WheelDirection,
    pub count: usize,
}

static SGR_WHEEL_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\x1b\[<([0-9]+);[0-9]+;[0-9]+(?:;[0-9]+;[0-9]+)?[mM]").unwrap_or_else(|_| unreachable!())
});

/// JS `ToInt32` of a number.
fn to_int32(value: f64) -> i32 {
    if !value.is_finite() {
        return 0;
    }
    let wrapped = value.trunc().rem_euclid(4_294_967_296.0);
    let wrapped = if wrapped >= 2_147_483_648.0 {
        wrapped - 4_294_967_296.0
    } else {
        wrapped
    };
    wrapped as i32
}

/// TS: app.tsx parseMouseWheel: the net wheel movement in a chunk of raw input (SGR and legacy reports).
pub fn parse_mouse_wheel(input: &str) -> Option<MouseWheel> {
    let mut up = 0usize;
    let mut down = 0usize;
    let mut count = |button: i32| {
        if button & 64 == 64 {
            if button & 1 == 1 {
                down += 1;
            } else {
                up += 1;
            }
        }
    };
    for captures in SGR_WHEEL_PATTERN.captures_iter(input) {
        let button = captures
            .get(1)
            .and_then(|digits| digits.as_str().parse::<f64>().ok())
            .unwrap_or(f64::NAN);
        count(to_int32(button));
    }
    // Legacy X10 reports: ESC [ M, then the button, x and y as single code units offset by 32.
    let units: Vec<u16> = input.encode_utf16().collect();
    let marker = [0x1b, u16::from(b'['), u16::from(b'M')];
    let find = |from: usize| {
        units
            .get(from..)
            .and_then(|rest| rest.windows(3).position(|window| window == marker))
            .map(|position| position + from)
    };
    let mut legacy = find(0);
    while let Some(index) = legacy {
        if units.len() < index + 6 {
            break;
        }
        count(i32::from(units[index + 3]) - 32);
        legacy = find(index + 6);
    }
    if up == 0 && down == 0 {
        return None;
    }
    Some(if up > down {
        MouseWheel {
            direction: WheelDirection::Up,
            count: up - down,
        }
    } else {
        MouseWheel {
            direction: WheelDirection::Down,
            count: down - up,
        }
    })
}

static ORPHAN_TERMINAL_SEQUENCE_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:\[(?:[ABCDHFIOZ]|[0-9]+(?:;[0-9]+)*[~ABCDHF])|O[ABCDHF])$").unwrap_or_else(|_| unreachable!())
});

static ORPHAN_MOUSE_SEQUENCE_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:\[?<[0-9]*(?:;[0-9]*){0,2}[mM]?|\[?[0-9]+;[0-9]*(?:;[0-9]*)?[mM]?)$")
        .unwrap_or_else(|_| unreachable!())
});

/// TS ORPHAN_MOUSE_SEQUENCE_PATTERN's last alternative, `\[?M[\s\S]{0,3}`: up to three UTF-16 code units after `M`.
fn orphan_legacy_mouse(text: &str) -> bool {
    let rest = text.strip_prefix('[').unwrap_or(text);
    rest.strip_prefix('M').is_some_and(|tail| js_len(tail) <= 3)
        || text.strip_prefix('M').is_some_and(|tail| js_len(tail) <= 3)
}

/// The characters a typed name keeps (TS ALLOWED_NAME_INPUT_PATTERN's complement).
fn allowed_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, ' ' | '_' | '-' | '/' | '.' | ':' | '[' | ']' | '(' | ')' | '#')
}

/// TS: app.tsx sanitizeNameInput: typed name text without controls, orphaned escape/mouse fragments or characters
/// outside `a-zA-Z0-9 _-/.:[]()#`.
pub fn sanitize_name_input(input: &str) -> String {
    let cleaned = strip_terminal_controls(input);
    if ORPHAN_TERMINAL_SEQUENCE_PATTERN.is_match(&cleaned)
        || ORPHAN_MOUSE_SEQUENCE_PATTERN.is_match(&cleaned)
        || orphan_legacy_mouse(&cleaned)
    {
        return String::new();
    }
    cleaned.chars().filter(|c| allowed_name_char(*c)).collect()
}

/// TS: app.tsx sortSessions.
pub fn sort_sessions(sessions: Vec<SessionRecord>) -> Vec<SessionRecord> {
    sort_sessions_for_sidebar(&sessions).into_iter().cloned().collect()
}

/// TS: app.tsx upsertSession: `session` replacing its old record, then sorted.
pub fn upsert_session(existing: Vec<SessionRecord>, session: SessionRecord) -> Vec<SessionRecord> {
    let mut next: Vec<SessionRecord> = existing.into_iter().filter(|item| item.id != session.id).collect();
    next.push(session);
    sort_sessions(next)
}

/// TS: app.tsx describeConnection (`ready` / `connecting…`).
pub fn describe_connection(connected: bool) -> &'static str {
    if connected { "ready" } else { "connecting…" }
}

/// TS: app.tsx connectionColor.
pub fn connection_color(connected: bool) -> Color {
    if connected { THEME.success } else { THEME.warn }
}

/// TS: app.tsx supportsForkedSubSession.
pub fn supports_forked_sub_session(session: Option<&SessionRecord>) -> bool {
    session.is_some_and(|session| agent_spec(session.program).forks)
}

/// TS: app.tsx forkStaysInParent: forks of an agent that may reopen them in the parent's directory run there.
pub fn fork_stays_in_parent(program: Option<ProgramKey>, sub_session_kind: Option<SubSessionKind>) -> bool {
    sub_session_kind == Some(SubSessionKind::Forked)
        && program.is_some_and(|program| !agent_spec(program).forks_across_directories)
}

/// TS: app.tsx parentWorkspaceLabel (`parent worktree: …` / `parent dir: …`).
pub fn parent_workspace_label(session: Option<&SessionRecord>, width: usize) -> Option<String> {
    let session = session?;
    if let Some(worktree) = session.worktree.as_ref()
        && truthy(worktree.path.as_ref())
        && worktree.mode != SessionWorktreeMode::None
    {
        let budget = width.saturating_sub(18).max(8);
        let name = if truthy(worktree.branch.as_ref()) {
            worktree.branch.clone().unwrap_or_default()
        } else if truthy(worktree.name.as_ref()) {
            worktree.name.clone().unwrap_or_default()
        } else {
            compact_path(worktree.path.as_deref().unwrap_or_default(), budget)
        };
        return Some(format!("parent worktree: {}", truncate(&name, budget)));
    }
    Some(format!(
        "parent dir: {}",
        compact_path(&session.cwd, width.saturating_sub(12).max(8))
    ))
}

/// TS: app.tsx sessionTitleFromTask: a session title (at most 64 characters) from a task's, cut at a word.
pub fn session_title_from_task(title: &str) -> String {
    // Sanitized text keeps only ASCII, where the only whitespace left is the space.
    let sanitized = sanitize_name_input(title);
    let clean = sanitized
        .split(' ')
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if clean.len() <= 64 {
        return clean;
    }
    let cut = &clean[..64];
    match cut.rfind(' ') {
        Some(space) if space >= 32 => cut[..space].trim().to_owned(),
        _ => cut.trim().to_owned(),
    }
}

/// TS: app.tsx baseOptions entries.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BaseOption {
    pub label: String,
    pub value: Option<String>,
}

/// TS: app.tsx baseOptions: where a new worktree's branch can start (the setting first, then local branches).
pub fn base_options(list: Option<&BranchList>) -> Vec<BaseOption> {
    let label = |text: String| BaseOption {
        label: text,
        value: None,
    };
    let Some(list) = list else {
        return vec![label("loading branches…".to_owned())];
    };
    if list.hook == Some(true) {
        return vec![label("decided by the repository's worktree hook".to_owned())];
    }
    let setting = match list.branch_from {
        BranchFrom::Default => format!(
            "{} (default branch)",
            list.default_branch.as_deref().unwrap_or("the default branch")
        ),
        BranchFrom::Origin => format!(
            "origin/{} (fetched first)",
            list.default_branch.as_deref().unwrap_or("main")
        ),
        BranchFrom::Current => match list.current.as_deref() {
            Some(current) if !current.is_empty() => format!("current checkout ({current})"),
            _ => "current checkout".to_owned(),
        },
    };
    let mut options = vec![label(setting)];
    options.extend(list.branches.iter().map(|name| BaseOption {
        label: name.clone(),
        value: Some(name.clone()),
    }));
    options
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

/// The derived values of one render (TS `sessions`, `visibleSessions`, `selectedSession`), computed once.
#[derive(Clone, Debug, Default)]
pub struct Derived {
    /// This checkout's sessions, replies read before the last detach marked read.
    pub sessions: Vec<SessionRecord>,
    /// The filtered, searched and (when applied) collapsed list.
    pub visible: Vec<SessionRecord>,
    /// The selected session when it is visible.
    pub selected: Option<SessionRecord>,
}

/// A hint part with a short form (TS `{text, short}`).
pub(crate) fn hint_short(text: &str, short: &str) -> HintPart {
    HintPart {
        text: text.to_owned(),
        short: Some(short.to_owned()),
        drop: None,
    }
}

/// Locators of the flows inside the app (for [`FlowCx`]).
fn agents_flow(app: &mut App) -> &mut AgentsFlow {
    &mut app.flows.agents
}
fn changes_flow(app: &mut App) -> &mut ChangesFlow {
    &mut app.flows.changes
}
fn notes_flow(app: &mut App) -> &mut NotesFlow {
    &mut app.flows.notes
}
fn settings_flow(app: &mut App) -> &mut SettingsFlow {
    &mut app.flows.settings
}
fn tasks_flow(app: &mut App) -> &mut TasksFlow {
    &mut app.flows.tasks
}
fn worktrees_flow(app: &mut App) -> &mut WorktreesFlow {
    &mut app.flows.worktrees
}

/// The task-link lookup the Notes flow gets (TS `tasksDoc ? taskLinkLookup(tasks) : undefined`).
pub(crate) fn notes_links(app: &App) -> Option<OwnedTaskLinkLookup> {
    if app.data.tasks_doc.is_some() {
        task_link_lookup(Some(&app.data.tasks))
    } else {
        None
    }
}

/// Builds a flow's props from the app's fields (borrowing them, not the flows) and calls `$call` with the flow.
macro_rules! with_props {
    (agents, $app:expr, $call:expr) => {{
        let app: &mut $crate::app::state::App = $app;
        let props = $crate::flows::agents::AgentsFlowProps {
            client: app.connection.client.as_ref(),
            versions: app.data.agent_versions.as_ref(),
        };
        ($call)(&mut app.flows.agents, &props)
    }};
    (settings, $app:expr, $call:expr) => {{
        let app: &mut $crate::app::state::App = $app;
        let props = $crate::flows::settings::SettingsFlowProps {
            client: app.connection.client.as_ref(),
            mode: app.mode,
        };
        ($call)(&mut app.flows.settings, &props)
    }};
    (changes, $app:expr, $call:expr) => {{
        let app: &mut $crate::app::state::App = $app;
        let selected = app.selected_session();
        let props = $crate::flows::changes::ChangesFlowProps {
            client: app.connection.client.as_ref(),
            session: selected.as_ref(),
            changes: &app.panes.changes,
            focused: app.mode == $crate::app::state::Mode::ChangesFocus,
        };
        ($call)(&mut app.flows.changes, &props)
    }};
    (notes, $app:expr, $call:expr) => {{
        let app: &mut $crate::app::state::App = $app;
        let derived = app.derived();
        let refs: Vec<&deckhand_protocol::data::SessionRecord> = derived.sessions.iter().collect();
        let lookup = $crate::app::notes_links(app);
        let links: Option<deckhand_core::notes::TaskLinkLookup<'_>> = lookup.as_deref();
        let props = $crate::flows::notes::NotesFlowProps {
            client: app.connection.client.as_ref(),
            session: derived.selected.as_ref(),
            sessions: &refs,
            focused: app.mode == $crate::app::state::Mode::NotesFocus,
            links,
        };
        ($call)(&mut app.flows.notes, &props)
    }};
    (tasks, $app:expr, $call:expr) => {{
        let app: &mut $crate::app::state::App = $app;
        let frame = app.spinner_frame();
        let refs: Vec<&deckhand_protocol::data::SessionRecord> = app.data.repo_sessions.iter().collect();
        let props = $crate::flows::tasks::TasksFlowProps {
            client: app.connection.client.as_ref(),
            repo_root: &app.props.repo_root,
            doc: app.data.tasks_doc.as_ref(),
            tasks: &app.data.tasks,
            sessions: &refs,
            spinner_frame: frame,
        };
        ($call)(&mut app.flows.tasks, &props)
    }};
    (worktrees, $app:expr, $call:expr) => {{
        let app: &mut $crate::app::state::App = $app;
        let frame = app.spinner_frame();
        let refs: Vec<&deckhand_protocol::data::SessionRecord> = app.data.repo_sessions.iter().collect();
        let props = $crate::flows::worktrees::WorktreesFlowProps {
            client: app.connection.client.as_ref(),
            cwd: &app.props.repo_root,
            sessions: &refs,
            spinner_frame: frame,
        };
        ($call)(&mut app.flows.worktrees, &props)
    }};
}
pub(crate) use with_props;

impl App {
    /// TS `sessions`: this checkout's sessions, replies read before the last detach marked read.
    pub fn sessions(&self) -> Vec<SessionRecord> {
        let none = IndexMap::new();
        let read_replies = self.props.read_replies.as_ref().unwrap_or(&none);
        self.data
            .repo_sessions
            .iter()
            .filter(|session| session_matches_scope(session, &self.props.repo_root))
            .map(|session| with_reply_read(session.clone(), read_replies))
            .collect()
    }

    /// TS `collapseApplied`: search and non-default filters reveal matches regardless of collapse state.
    pub fn collapse_applied(&self) -> bool {
        self.browse.session_query.is_empty() && self.browse.session_filter == SessionFilter::Active
    }

    /// The title of the tasks a session works on (TS filterSessionList's `taskTitle`).
    fn task_titles(&self, session: &SessionRecord) -> Option<String> {
        Some(
            linked_tasks(&self.data.tasks, Some(session))
                .iter()
                .map(|task| task.title.as_str())
                .collect::<Vec<_>>()
                .join(" "),
        )
    }

    /// TS `visibleSessions` of `sessions`.
    pub fn visible_of(&self, sessions: &[SessionRecord]) -> Vec<SessionRecord> {
        let title: TaskTitleOf<'_> = &|session: &SessionRecord| self.task_titles(session);
        let filtered = filter_session_list(
            sessions,
            self.browse.session_filter,
            &self.browse.session_query,
            Some(title),
        );
        if !self.collapse_applied() {
            return filtered.into_iter().cloned().collect();
        }
        let collapsed: HashSet<String> = self.browse.collapsed_session_ids.iter().cloned().collect();
        let hidden: HashSet<String> = self.browse.hidden_exited_session_ids.iter().cloned().collect();
        filter_collapsed_sessions(&filtered, &collapsed, Some(&hidden))
            .into_iter()
            .map(|session| (**session).clone())
            .collect()
    }

    /// TS `visibleSessions`: the filtered, searched and (when applied) collapsed list.
    pub fn visible_sessions(&self) -> Vec<SessionRecord> {
        self.visible_of(&self.sessions())
    }

    /// `sessions`, `visibleSessions` and `selectedSession` at once.
    pub fn derived(&self) -> Derived {
        let sessions = self.sessions();
        let visible = self.visible_of(&sessions);
        let selected = self
            .browse
            .selected_id
            .as_ref()
            .and_then(|id| visible.iter().find(|session| &session.id == id).cloned());
        Derived {
            sessions,
            visible,
            selected,
        }
    }

    /// TS `selectedIndex` (0 without a visible selection).
    pub fn selected_index(&self) -> usize {
        let Some(id) = self.browse.selected_id.as_ref() else {
            return 0;
        };
        self.visible_sessions()
            .iter()
            .position(|session| &session.id == id)
            .unwrap_or(0)
    }

    /// TS `selectedSession`: only a visible session is actionable.
    pub fn selected_session(&self) -> Option<SessionRecord> {
        let id = self.browse.selected_id.as_ref()?;
        self.visible_sessions().into_iter().find(|session| &session.id == id)
    }

    /// TS `layout` (the memo of the terminal size and the sidebar override).
    pub fn layout(&self) -> AppLayout {
        app_layout(self.browse.terminal_size, self.browse.sidebar_width_override)
    }

    /// TS `currentWorkspaceInfo`.
    pub fn current_workspace_info(&self) -> Option<&WorkspaceInfo> {
        let selected = self.selected_session();
        self.workspace_info
            .info
            .as_ref()
            .filter(|info| selected.as_ref().is_some_and(|session| session.id == info.session_id))
    }

    /// TS `currentCleanupCheck`.
    pub fn current_cleanup_check(&self) -> Option<&CleanupCheck> {
        let selected = self.selected_session();
        self.kill
            .cleanup_check
            .as_ref()
            .filter(|check| selected.as_ref().is_some_and(|session| session.id == check.session_id))
    }

    /// TS `cleanupInspectionFor(deleteBranch)`.
    pub fn cleanup_inspection_for(&self, delete_branch: bool) -> Option<&SessionCleanupInspection> {
        let check = self.current_cleanup_check()?;
        if delete_branch {
            check.branch.as_ref()
        } else {
            check.worktree.as_ref()
        }
    }

    /// TS `showingAction`: the Terminal tab shows the workspace's last action.
    pub fn showing_action(&self) -> bool {
        self.browse.active_tab == RightPaneTab::Terminal
            && self.panes.terminal_view == TerminalView::Action
            && has_action(self.selected_session().as_ref(), &self.panes.action)
    }

    /// TS `activeAttachTarget`.
    pub fn active_attach_target(&self) -> AttachTarget {
        if self.showing_action() {
            return AttachTarget::Action;
        }
        match self.browse.active_tab {
            RightPaneTab::Terminal => AttachTarget::Terminal,
            RightPaneTab::Git => AttachTarget::Git,
            RightPaneTab::Dev => AttachTarget::Dev,
            RightPaneTab::Preview | RightPaneTab::Notes => AttachTarget::Agent,
        }
    }

    /// TS `selectedWorkspace`.
    pub fn selected_workspace(&self) -> Option<String> {
        self.selected_session().as_ref().and_then(workspace_key)
    }

    /// TS `runListHasDev`.
    pub fn run_list_has_dev(&self) -> bool {
        let Some(session_id) = self
            .action_picker
            .action_project
            .as_ref()
            .and_then(|project| project.session_id.as_ref())
            .filter(|id| !id.is_empty())
        else {
            return false;
        };
        let selected = self.selected_session();
        selected.as_ref().is_some_and(|session| &session.id == session_id)
            && selected
                .as_ref()
                .and_then(workspace_key)
                .is_some_and(|key| !key.is_empty())
    }

    /// TS `activePaneReadyForAttach`.
    pub fn active_pane_ready_for_attach(&self) -> bool {
        let Some(session) = self.selected_session() else {
            return false;
        };
        let mine = |id: &Option<String>| id.as_deref() == Some(session.id.as_str());
        match self.active_attach_target() {
            AttachTarget::Agent => session.status == SessionStatus::Running,
            AttachTarget::Terminal => mine(&self.panes.terminal.session_id) && self.panes.terminal.live,
            // lazygit starts on attach; the Changes record confirms the daemon sees the workspace.
            AttachTarget::Git => mine(&self.panes.changes.session_id) && truthy(self.panes.changes.workspace.as_ref()),
            AttachTarget::Dev => mine(&self.panes.dev.session_id) && self.panes.dev.live,
            AttachTarget::Action => self.panes.action.live,
        }
    }

    /// TS `selectedCanDeleteWorktree`.
    pub fn selected_can_delete_worktree(&self) -> bool {
        let Some(session) = self.selected_session() else {
            return false;
        };
        let Some(worktree) = session.worktree.as_ref() else {
            return false;
        };
        truthy(worktree.path.as_ref())
            && worktree.mode != SessionWorktreeMode::None
            && !truthy(worktree.deleted_at.as_ref())
            && worktree.is_main != Some(true)
            && (!truthy(session.launch_worktree_root.as_ref()) || worktree.path != session.launch_worktree_root)
            && !self.sessions().iter().any(|other| {
                other.id != session.id
                    && other.status != SessionStatus::Exited
                    && other.worktree.as_ref().and_then(|record| record.path.as_ref()) == worktree.path.as_ref()
            })
    }

    /// TS `selectedCanDeleteBranch`.
    pub fn selected_can_delete_branch(&self) -> bool {
        if !self.selected_can_delete_worktree() {
            return false;
        }
        let selected = self.selected_session();
        let branch = selected
            .as_ref()
            .and_then(|session| session.worktree.as_ref())
            .and_then(|worktree| worktree.branch.as_deref())
            .unwrap_or("");
        !branch.is_empty() && branch != "main" && branch != "master"
    }

    /// TS `spinnerFrame`.
    pub fn spinner_frame(&self) -> &'static str {
        SPINNER_FRAMES
            .get(self.browse.spinner_index)
            .copied()
            .unwrap_or(SPINNER_FRAMES[0])
    }

    /// TS `setMode`.
    pub fn set_mode(&mut self, mode: Mode) {
        self.mode = mode;
    }

    /// TS `setError` (auto-hidden after ERROR_MESSAGE_AUTO_HIDE_MS by the error effect).
    pub fn set_error(&mut self, error: Option<String>) {
        self.messages.error = error;
    }

    /// TS `setStatusMessage` (auto-hidden after STATUS_MESSAGE_AUTO_HIDE_MS by the status effect).
    pub fn set_status(&mut self, status: Option<String>) {
        self.messages.status_message = status;
    }

    /// TS `setBusy`.
    pub fn set_busy(&mut self, busy: bool) {
        self.messages.busy = busy;
    }

    /// TS `setSelectedId`. The selected-id effect (reset the scroll, restore the session's tab, report it) runs in the
    /// effect phase after the update ([`effects::run_effects`]).
    pub fn set_selected_id(&mut self, id: Option<String>) {
        self.browse.selected_id = id;
    }

    /// TS `setActiveTab`. The active-tab effect (remember it for the session, leave tab-bound focus modes) runs in
    /// the effect phase after the update.
    pub fn set_active_tab(&mut self, tab: RightPaneTab) {
        self.browse.active_tab = tab;
    }

    /// TS `setTasksDoc`, with `tasks` reparsed when the text changed (TS: the `[tasksDoc?.text]` memo).
    pub fn set_tasks_doc(&mut self, doc: Option<TasksDoc>) {
        let text = |doc: Option<&TasksDoc>| doc.map(|doc| doc.text.clone()).unwrap_or_default();
        let changed = text(self.data.tasks_doc.as_ref()) != text(doc.as_ref());
        self.data.tasks_doc = doc;
        if changed {
            self.data.tasks = parse_tasks(&text(self.data.tasks_doc.as_ref()));
        }
    }

    /// TS `setSessions(current => upsertSession(current, session))`.
    pub fn upsert(&mut self, session: SessionRecord) {
        let current = std::mem::take(&mut self.data.repo_sessions);
        self.data.repo_sessions = upsert_session(current, session);
    }

    /// TS: the five pane records back to empty (session removed, none selected).
    pub fn reset_panes(&mut self) {
        self.panes.preview = empty_preview();
        self.panes.terminal = empty_pane();
        self.panes.changes = deckhand_core::changes_model::empty_changes();
        self.panes.dev = empty_pane();
        self.panes.action = empty_pane();
    }

    /// TS `exit(result)` (useApp): the loop ends with it.
    pub fn exit_with(&mut self, result: UiExitResult) {
        // Ink resolves waitUntilExit once; a second exit changes nothing.
        if self.connection.exit.is_none() {
            self.connection.exit = Some(result);
        }
    }

    /// TS `quit` (q, Ctrl+C).
    pub fn quit(&mut self) {
        self.exit_with(UiExitResult::Quit);
    }

    /// TS `attachTo`: exit to the attach hand-off for `session`'s `target`.
    pub fn attach_to(&mut self, session: &SessionRecord, target: AttachTarget) {
        let title = display_session_title(session, &self.sessions());
        self.exit_with(UiExitResult::Attach {
            session_id: session.id.clone(),
            target,
            title: Some(title),
            cwd: Some(session.cwd.clone()),
            program: Some(session.program),
        });
    }

    /// TS `elsewhere`: a session of another checkout is in the sidebar of the Deckhand opened there (says so).
    pub fn elsewhere(&mut self, session_id: &str) -> bool {
        let Some(target) = self.data.repo_sessions.iter().find(|session| session.id == session_id) else {
            return false;
        };
        if self.sessions().iter().any(|session| session.id == session_id) {
            return false;
        }
        let message = format!(
            "{} is in the sidebar of the Deckhand opened in {}",
            display_session_title(target, &self.data.repo_sessions),
            compact_path(&target.repo_root, 60)
        );
        self.set_status(Some(message));
        true
    }

    /// TS `revealSession`: clears the search, widens a filter that hides it and expands its collapsed ancestors.
    pub fn reveal_session(&mut self, session_id: &str) {
        let sessions = self.sessions();
        self.browse.session_query.clear();
        let Some(target) = sessions.iter().find(|session| session.id == session_id) else {
            return;
        };
        let current = self.browse.session_filter;
        let filter = if truthy(target.archived_at.as_ref()) {
            SessionFilter::All
        } else if filter_session_list(&sessions, current, "", None)
            .iter()
            .any(|session| session.id == session_id)
        {
            current
        } else {
            SessionFilter::Active
        };
        self.browse.session_filter = filter;
        if filter != SessionFilter::Active {
            return;
        }
        let mut lineage: HashSet<String> = HashSet::from([session_id.to_owned()]);
        let mut parent = target.parent_session_id.clone();
        while let Some(id) = parent.filter(|id| !id.is_empty() && !lineage.contains(id)) {
            parent = sessions
                .iter()
                .find(|session| session.id == id)
                .and_then(|session| session.parent_session_id.clone());
            lineage.insert(id);
        }
        self.browse.collapsed_session_ids.retain(|id| !lineage.contains(id));
        self.browse.hidden_exited_session_ids.retain(|id| !lineage.contains(id));
    }

    /// TS `goToSession` (o on a task, enter on a worktree): reveal and select it, back to browse.
    pub fn go_to_session(&mut self, session_id: &str) {
        if self.elsewhere(session_id) {
            return;
        }
        self.reveal_session(session_id);
        self.set_selected_id(Some(session_id.to_owned()));
        self.set_mode(Mode::Browse);
    }

    /// TS: tasksFlow's `onOpenNote`: that session's Notes tab.
    pub fn open_note(&mut self, session_id: &str) {
        if self.elsewhere(session_id) {
            return;
        }
        self.reveal_session(session_id);
        self.browse
            .session_tabs
            .insert(session_id.to_owned(), RightPaneTab::Notes);
        self.set_selected_id(Some(session_id.to_owned()));
        self.set_active_tab(RightPaneTab::Notes);
        self.set_mode(Mode::Browse);
    }

    /// TS `editNoteInTerminal` (o on Notes): exit to `$EDITOR` on the active note.
    pub fn edit_note_in_terminal(&mut self, session: &SessionRecord) {
        let Some(editor) = resolve_terminal_editor(None) else {
            self.set_error(Some(
                "No terminal editor found: set $EDITOR or install nvim (E opens the note in Cursor / VS Code)"
                    .to_owned(),
            ));
            return;
        };
        let session_id = session.id.clone();
        let then: FileThen = Box::new(move |app: &mut App, file: Option<String>| {
            if let Some(file) = file {
                app.exit_with(UiExitResult::EditNote {
                    session_id,
                    file,
                    editor,
                });
            }
        });
        self.notes_flow_call(|flow, props, cx| flow.active_file(props, cx, then));
    }

    /// TS `reviewThen`: resolves the repository config for `target_cwd` and runs `resume` with it, showing the
    /// inline review first when `gate` says the action depends on an untrusted override.
    pub fn review_then(
        &mut self,
        target_cwd: &str,
        resume: Option<AppAction<Option<ProjectInfo>>>,
        options: ReviewOptions,
    ) {
        let Some(client) = self.client() else {
            return;
        };
        self.set_busy(true);
        self.set_error(None);
        let reply = client.project_info(target_cwd);
        let cwd = target_cwd.to_owned();
        let task = self.dispatch().spawn(reply, move |app: &mut App, result| match result {
            Ok(project) => {
                app.set_busy(false);
                let gated = match &options.gate {
                    Some(gate) => gate(&project),
                    None => project.needs_review,
                };
                if let Some(resume) = &resume
                    && !gated
                {
                    resume(app, Some(project));
                    return;
                }
                let labels = options.labels.as_ref().map(|labels| labels(&project));
                let purpose = options.purpose.as_ref().map(|purpose| purpose(&project));
                app.review.review = Some(Review {
                    project,
                    cwd,
                    resume,
                    skip: options.skip,
                    labels,
                    purpose,
                    back: options.back,
                });
                app.review.details_scroll = 0;
                app.set_mode(Mode::ReviewProject);
            }
            Err(error) => {
                app.set_busy(false);
                app.set_error(Some(error.to_string()));
                if !options.required
                    && let Some(resume) = resume
                {
                    resume(app, None);
                }
            }
        });
        drop(task);
    }

    /// TS `detailsContent`: title, text and footer of the scrollable details modes.
    pub fn details_content(&self) -> Option<DetailsContent> {
        let scroll = self.review.details_scroll;
        let content = |title: &str, text: String, footer: Vec<HintPart>| DetailsContent {
            title: title.to_owned(),
            text,
            footer,
            scroll,
        };
        match self.mode {
            Mode::WorkspaceInfo => {
                let info = self.current_workspace_info();
                if let Some(info) = info
                    && info.confirm_pr
                    && let Some(summary) = info.summary.as_ref()
                {
                    return Some(content(
                        "Create pull request",
                        create_pr_confirm_text(summary),
                        vec!["enter push & open PR form".into(), "esc cancel".into()],
                    ));
                }
                let links = self
                    .selected_session()
                    .and_then(|session| session.worktree)
                    .and_then(|worktree| worktree.links);
                let text = workspace_summary_text(
                    info.and_then(|info| info.summary.as_ref()),
                    info.is_some_and(|info| info.pr_loading),
                    &WorkspaceSummaryOptions {
                        creating_pr: info.is_some_and(|info| info.creating_pr),
                        links,
                    },
                );
                Some(content(
                    "Workspace overview",
                    text,
                    vec![
                        hint_short("P fetch PR status", "P PR status"),
                        "b open PR".into(),
                        "c create PR".into(),
                        "g git tab".into(),
                        "esc close".into(),
                    ],
                ))
            }
            Mode::ReviewProject => {
                let review = self.review.review.as_ref();
                let text = match review {
                    Some(review) => trust_review_text(&review.project, review.purpose.as_deref()),
                    None => "No repository configuration loaded.".to_owned(),
                };
                let footer = match review {
                    Some(review) if review.resume.is_some() => {
                        let labels = review.labels.clone().unwrap_or_default();
                        vec![
                            labels
                                .enter
                                .unwrap_or_else(|| hint_short("enter trust & continue", "enter trust")),
                            labels.skip.unwrap_or_else(|| match review.skip {
                                ReviewSkip::Cancel => "s cancel".into(),
                                _ => {
                                    hint_short("s continue without it (global defaults only)", "s continue without it")
                                }
                            }),
                            if review.back.is_some() {
                                "esc back".into()
                            } else {
                                "esc cancel".into()
                            },
                        ]
                    }
                    Some(review) if review.project.needs_review => vec!["enter trust".into(), "esc close".into()],
                    _ => vec!["esc close".into()],
                };
                Some(content("Review repository configuration", text, footer))
            }
            Mode::ConfirmLoss => {
                let inspection = self.cleanup_inspection_for(self.kill.pending_delete_branch);
                Some(content(
                    "Destructive cleanup override",
                    cleanup_override_text(inspection.map(|inspection| &inspection.inspection)),
                    vec![
                        HintPart::from(format!("Type DELETE then enter: {}", self.kill.confirmation_draft).as_str()),
                        "esc cancel".into(),
                    ],
                ))
            }
            _ => None,
        }
    }

    /// TS `scrollDetailsPane`: shared scrolling of the details modes; true when the key scrolled.
    pub fn scroll_details_pane(&mut self, input: &Input) -> bool {
        let Some(content) = self.details_content() else {
            return false;
        };
        let layout = self.layout();
        let viewport = details_viewport(
            &content.text,
            usize::from(layout.preview_width),
            usize::from(layout.content_height),
        );
        match scroll_details(self.review.details_scroll, input, viewport.rows, viewport.max) {
            Some(next) => {
                self.review.details_scroll = next;
                true
            }
            None => false,
        }
    }

    /// TS: the useTerminalInput handler: Ctrl+C, the busy gate, then the mode's keys (crates/ui/src/app/modes).
    pub fn handle_input(&mut self, input: &Input) {
        use crate::app::modes::{
            action_picker, browse, confirm, create, filter, preview_focus, review, search, workspace_info,
        };
        let key = &input.key;
        // Ink's exitOnCtrlC is off (cli.ts) so a first Ctrl+C acts like Esc while a Settings edit or JSON draft is
        // open (asking before discarding it). Ctrl+C at the discard prompt, while a save is in flight, or anywhere
        // else quits like q.
        if key.ctrl && input.input == "c" {
            let cancelable = with_props!(
                settings,
                self,
                |flow: &mut SettingsFlow, props: &SettingsFlowProps<'_>| { flow.cancelable(props) }
            );
            if cancelable && !self.messages.busy {
                let escape = Input::key(
                    "",
                    Key {
                        escape: true,
                        ..Key::NONE
                    },
                );
                self.settings_flow_call(|flow, props, cx| flow.handle_input(props, &escape, cx));
                return;
            }
            self.quit();
            return;
        }
        if self.messages.busy {
            return;
        }
        let mode = self.mode;
        if is_settings_flow_mode(mode) {
            self.settings_flow_call(|flow, props, cx| flow.handle_input(props, input, cx));
            return;
        }
        match mode {
            Mode::Agents => self.agents_flow_call(|flow, props, cx| flow.handle_input(props, input, cx)),
            Mode::Help => {
                if self.flows.help.handle_input(input) == Some(HelpOutcome::Close) {
                    self.set_mode(Mode::Browse);
                }
            }
            Mode::PreviewFocus => preview_focus::handle_preview_focus(self, input),
            Mode::Tasks => self.tasks_flow_call(|flow, props, cx| flow.handle_input(props, input, cx)),
            Mode::Worktrees => self.worktrees_flow_call(|flow, props, cx| flow.handle_input(props, input, cx)),
            Mode::ConfirmRemove => confirm::handle_confirm_remove(self, input),
            Mode::ChangesFocus => preview_focus::handle_changes_focus(self, input),
            Mode::NotesFocus => preview_focus::handle_notes_focus(self, input),
            Mode::PickFilter => {
                let back = self.filter.clone();
                filter::pick_filter(self, input, back.as_ref());
            }
            Mode::Search => search::handle_search(self, input),
            Mode::ReviewProject => review::handle_review_project(self, input),
            Mode::ConfirmLoss => confirm::handle_confirm_loss(self, input),
            Mode::WorkspaceInfo => workspace_info::handle_workspace_info(self, input),
            Mode::PickAction => action_picker::handle_pick_action(self, input),
            Mode::Browse => browse::handle_browse(self, input),
            Mode::PickProgram => create::handle_pick_program(self, input),
            Mode::EnterName => create::handle_enter_name(self, input),
            Mode::PickWorktree => create::handle_pick_worktree(self, input),
            Mode::MergeConflicts => confirm::handle_merge_conflicts(self, input),
            Mode::ConfirmMerge => confirm::handle_confirm_merge(self, input),
            Mode::ConfirmKill => confirm::handle_confirm_kill(self, input),
            Mode::Settings | Mode::EditProject | Mode::DiscardProject => {}
        }
    }

    /// Calls the Agents flow with its props and a fresh cx, then applies its effects.
    pub fn agents_flow_call<R>(
        &mut self,
        call: impl FnOnce(&mut AgentsFlow, &AgentsFlowProps<'_>, &mut FlowCx<App, AgentsFlow>) -> R,
    ) -> R {
        let mut cx = FlowCx::new(self.dispatch().clone(), agents_flow);
        let result = with_props!(agents, self, |flow: &mut AgentsFlow, props: &AgentsFlowProps<'_>| call(
            flow, props, &mut cx
        ));
        let effects = cx.take_effects();
        self.apply_flow_effects(effects);
        result
    }

    /// Calls the Changes flow with its props and a fresh cx, then applies its effects.
    pub fn changes_flow_call<R>(
        &mut self,
        call: impl FnOnce(&mut ChangesFlow, &ChangesFlowProps<'_>, &mut FlowCx<App, ChangesFlow>) -> R,
    ) -> R {
        let mut cx = FlowCx::new(self.dispatch().clone(), changes_flow);
        let result = with_props!(changes, self, |flow: &mut ChangesFlow, props: &ChangesFlowProps<'_>| {
            call(flow, props, &mut cx)
        });
        let effects = cx.take_effects();
        self.apply_flow_effects(effects);
        result
    }

    /// Calls the Notes flow with its props and a fresh cx, then applies its effects.
    pub fn notes_flow_call<R>(
        &mut self,
        call: impl FnOnce(&mut NotesFlow, &NotesFlowProps<'_>, &mut FlowCx<App, NotesFlow>) -> R,
    ) -> R {
        let mut cx = FlowCx::new(self.dispatch().clone(), notes_flow);
        let result = with_props!(notes, self, |flow: &mut NotesFlow, props: &NotesFlowProps<'_>| call(
            flow, props, &mut cx
        ));
        let effects = cx.take_effects();
        self.apply_flow_effects(effects);
        result
    }

    /// Calls the Settings flow with its props and a fresh cx, then applies its effects.
    pub fn settings_flow_call<R>(
        &mut self,
        call: impl FnOnce(&mut SettingsFlow, &SettingsFlowProps<'_>, &mut FlowCx<App, SettingsFlow>) -> R,
    ) -> R {
        let mut cx = FlowCx::new(self.dispatch().clone(), settings_flow);
        let result = with_props!(
            settings,
            self,
            |flow: &mut SettingsFlow, props: &SettingsFlowProps<'_>| call(flow, props, &mut cx)
        );
        let effects = cx.take_effects();
        self.apply_flow_effects(effects);
        result
    }

    /// Calls the Tasks flow with its props and a fresh cx, then applies its effects.
    pub fn tasks_flow_call<R>(
        &mut self,
        call: impl FnOnce(&mut TasksFlow, &TasksFlowProps<'_>, &mut FlowCx<App, TasksFlow>) -> R,
    ) -> R {
        let mut cx = FlowCx::new(self.dispatch().clone(), tasks_flow);
        let result = with_props!(tasks, self, |flow: &mut TasksFlow, props: &TasksFlowProps<'_>| call(
            flow, props, &mut cx
        ));
        let effects = cx.take_effects();
        self.apply_flow_effects(effects);
        result
    }

    /// Calls the Worktrees flow with its props and a fresh cx, then applies its effects.
    pub fn worktrees_flow_call<R>(
        &mut self,
        call: impl FnOnce(&mut WorktreesFlow, &WorktreesFlowProps<'_>, &mut FlowCx<App, WorktreesFlow>) -> R,
    ) -> R {
        let mut cx = FlowCx::new(self.dispatch().clone(), worktrees_flow);
        let result = with_props!(
            worktrees,
            self,
            |flow: &mut WorktreesFlow, props: &WorktreesFlowProps<'_>| call(flow, props, &mut cx)
        );
        let effects = cx.take_effects();
        self.apply_flow_effects(effects);
        result
    }

    /// The selection the daemon handlers compare with (TS `selectedIdRef.current`, set by the selected-id effect).
    pub fn selected_id_ref(&self) -> Option<String> {
        match &self.effects.selected_id {
            Some(id) => id.clone(),
            None => self.browse.selected_id.clone(),
        }
    }
}

/// `FnOnce` callbacks kept as an [`AppCallback`] (it runs once; later calls do nothing).
fn once_callback(callback: crate::flow::HostCallback<App>) -> AppCallback {
    let slot = RefCell::new(Some(callback));
    Rc::new(move |app: &mut App| {
        let callback = slot.borrow_mut().take();
        if let Some(callback) = callback {
            callback(app);
        }
    })
}

impl FlowHost for App {
    /// TS: the flows' callbacks (onExit → browse, setMode, setError, onGoTo → goToSession, ...).
    fn apply_flow_effects(&mut self, effects: Vec<FlowEffect<Self>>) {
        for effect in effects {
            match effect {
                FlowEffect::Exit => self.set_mode(Mode::Browse),
                FlowEffect::SetMode(mode) => self.set_mode(mode),
                FlowEffect::SetBusy(busy) => self.set_busy(busy),
                FlowEffect::SetError(error) => self.set_error(error),
                FlowEffect::SetStatus(status) => self.set_status(status),
                FlowEffect::Versions(versions) => self.data.agent_versions = Some(*versions),
                FlowEffect::Changes(record) => {
                    if record.session_id == self.selected_id_ref() {
                        self.panes.changes = *record;
                    }
                }
                FlowEffect::AttachGit => {
                    if let Some(session) = self.selected_session() {
                        self.attach_to(&session, AttachTarget::Git);
                    }
                }
                FlowEffect::TasksDoc(doc) => self.set_tasks_doc(Some(*doc)),
                FlowEffect::StartTask(task) => crate::app::modes::create::start_from_task(self, &task),
                FlowEffect::GoTo(session_id) => self.go_to_session(&session_id),
                FlowEffect::OpenNote(session_id) => self.open_note(&session_id),
                FlowEffect::Session(session) => self.upsert(*session),
                FlowEffect::Review { cwd, back } => {
                    let options = ReviewOptions {
                        back: Some(once_callback(back)),
                        ..ReviewOptions::default()
                    };
                    self.review_then(&cwd, None, options);
                }
            }
        }
    }
}

impl UiModel for App {
    type Exit = UiExitResult;

    /// TS: the preview-focus `process.stdin.on('data')` listener (mouse wheel).
    fn on_stdin_text(&mut self, text: &str) {
        if self.mode == Mode::PreviewFocus {
            crate::app::modes::preview_focus::on_mouse_data(self, text);
        }
    }

    /// TS: useTerminalInput (releases dropped, the key normalised) → [`App::handle_input`].
    fn on_key(&mut self, event: &KeyEvent) {
        if let Some(input) = terminal_input(event) {
            self.handle_input(&input);
        }
    }

    /// TS: the resize effect.
    fn on_resize(&mut self, cols: u16, rows: u16) {
        self.browse.terminal_size = crate::app::state::TerminalSize {
            cols: if cols == 0 { 80 } else { cols },
            rows: if rows == 0 { 24 } else { rows },
        };
    }

    /// TS: every `useEffect` of App (crates/ui/src/app/effects.rs).
    fn after_update(&mut self) {
        effects::run_effects(self);
    }

    /// TS: App's render (crates/ui/src/app/layout.rs).
    fn render(&mut self, frame: &mut Frame<'_>) {
        layout::render_app(self, frame);
    }

    fn exit(&mut self) -> Option<UiExitResult> {
        self.connection.exit.take()
    }
}

/// TS: cli.ts runUi's Ink part: renders App on the terminal (raw stdin, resize, the event loop) until it exits.
/// The caller (crates/deckhand/src/ui_loop.rs) owns the alternate screen, the UI state file and what follows the
/// exit (attach, `$EDITOR`).
pub async fn run(props: AppProps) -> io::Result<Option<UiExitResult>> {
    use crate::app::event_loop::{
        Dispatch, LoopEnd, StdinReader, capture_startup_termios, die_of_signal, enable_ink_raw_mode,
        spawn_resize_watcher, stdin_fd, terminal_size, termination_signal,
    };
    use crate::app::state::TerminalSize;

    let fd = stdin_fd();
    // A no-op when the process saved its startup termios already (crates/deckhand/src/ui_loop.rs does).
    capture_startup_termios(fd);
    let signalled = termination_signal()?;
    let raw = enable_ink_raw_mode(fd)?;
    let (tx, mut events) = tokio::sync::mpsc::unbounded_channel();
    let mut reader = StdinReader::spawn(fd, tx.clone())?;
    let resize = spawn_resize_watcher(tx)?;
    let (cols, rows) = terminal_size();
    let backend = event_loop::InkBackend::new(io::stdout(), event_loop::should_synchronize());
    let mut terminal = ratatui::Terminal::new(backend)?;
    terminal.hide_cursor()?;
    let (dispatch, mut updates) = Dispatch::channel();
    let mut app = App::new(props, dispatch, TerminalSize { cols, rows });
    app.effects.stdout_modes = true;
    // TS: Ink's signalExit unmounts the tree on SIGINT/SIGTERM/SIGHUP, then the process dies of the signal.
    let (end, signal) = tokio::select! {
        end = event_loop::run(&mut app, &mut terminal, &mut events, &mut updates) => (Some(end), None),
        signal = signalled => (None, Some(signal)),
    };
    // TS: the effects' cleanups on unmount (connection closed, terminal modes undone), then Ink's cleanup.
    effects::unmount(&mut app);
    resize.abort();
    reader.stop();
    drop(raw);
    let _ = terminal.show_cursor();
    if let Some(signal) = signal {
        die_of_signal(signal);
    }
    Ok(match end.unwrap_or(Ok(LoopEnd::InputClosed))? {
        LoopEnd::Exit(result) => Some(result),
        LoopEnd::InputClosed => None,
    })
}

#[cfg(test)]
pub(crate) mod test_support {
    //! An app for tests: no daemon (the connection and config effects marked as run), a 130×36 terminal.

    use super::*;
    use crate::app::event_loop::Dispatch;
    use crate::app::state::TerminalSize;
    use deckhand_protocol::data::AgentActivityStatus;

    /// A session of `/repo`.
    pub fn session(id: &str, title: &str, status: SessionStatus) -> SessionRecord {
        let value = serde_json::json!({
            "id": id,
            "title": title,
            "program": "claude",
            "command": "claude",
            "cwd": "/repo",
            "repoRoot": "/repo",
            "status": status.as_str(),
            "createdAt": "2026-01-01T00:00:00.000Z",
            "updatedAt": "2026-01-01T00:00:00.000Z",
        });
        let mut record: SessionRecord = serde_json::from_value(value).unwrap_or_else(|error| panic!("{error}"));
        if status == SessionStatus::Running {
            record.agent_status = Some(AgentActivityStatus::Idle);
        }
        record
    }

    /// An app for `/repo` whose mount and connection effects count as run; the receiver keeps updates.
    pub fn app(
        props: AppProps,
    ) -> (
        App,
        tokio::sync::mpsc::UnboundedReceiver<crate::app::event_loop::Update<App>>,
    ) {
        let (dispatch, rx) = Dispatch::channel();
        let props = AppProps {
            repo_root: "/repo".to_owned(),
            cwd: "/repo".to_owned(),
            ..props
        };
        let mut app = App::new(props, dispatch, TerminalSize { cols: 130, rows: 36 });
        app.effects.mounted = true;
        app.effects.connection = Some(0);
        (app, rx)
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::{app, session};
    use super::*;

    #[test]
    fn scroll_sensitivity_is_clamped_and_printed_like_to_fixed() {
        assert_eq!(normalize_scroll_sensitivity(None), 0.12);
        assert_eq!(normalize_scroll_sensitivity(Some(f64::NAN)), 0.12);
        assert_eq!(normalize_scroll_sensitivity(Some(2.0)), 1.0);
        assert_eq!(normalize_scroll_sensitivity(Some(-1.0)), 0.0);
        assert_eq!(format_scroll_sensitivity(0.12), "0.12");
        assert_eq!(format_scroll_sensitivity(0.1), "0.1");
        assert_eq!(format_scroll_sensitivity(1.0), "1");
        assert_eq!(format_scroll_sensitivity(0.0), "0");
        assert_eq!(format_scroll_sensitivity(0.16000000000000003), "0.16");
        // toFixed rounds an exact tie up (Rust's formatting would round it to even).
        assert_eq!(format_scroll_sensitivity(0.125), "0.13");
        assert_eq!(format_scroll_sensitivity(1.005), "1");
    }

    #[test]
    fn wheel_sequences_round_trip() {
        let up = mouse_wheel_sequence(WheelDirection::Up, 80, 20, 2);
        assert_eq!(up, "\u{1b}[<64;40;10M\u{1b}[<64;40;10M");
        assert_eq!(mouse_wheel_sequence(WheelDirection::Down, 1, 1, 0), "\u{1b}[<65;1;1M");
        assert_eq!(
            parse_mouse_wheel(&up),
            Some(MouseWheel {
                direction: WheelDirection::Up,
                count: 2
            })
        );
        // Legacy reports: button 96 (wheel up) / 97 (down) as code units.
        let legacy = "\u{1b}[M\u{61}!!\u{1b}[M\u{61}!!\u{1b}[M\u{60}!!";
        assert_eq!(
            parse_mouse_wheel(legacy),
            Some(MouseWheel {
                direction: WheelDirection::Down,
                count: 1
            })
        );
        assert_eq!(parse_mouse_wheel("abc"), None);
        assert_eq!(parse_mouse_wheel("\u{1b}[<0;1;1M"), None);
        // As many up as down: TS reports down by zero.
        assert_eq!(
            parse_mouse_wheel("\u{1b}[<64;1;1M\u{1b}[<65;1;1m"),
            Some(MouseWheel {
                direction: WheelDirection::Down,
                count: 0
            })
        );
    }

    #[test]
    fn name_input_drops_controls_and_orphaned_sequences() {
        assert_eq!(sanitize_name_input("fix bug #12"), "fix bug #12");
        assert_eq!(sanitize_name_input("[A"), "");
        assert_eq!(sanitize_name_input("[15~"), "");
        assert_eq!(sanitize_name_input("OB"), "");
        assert_eq!(sanitize_name_input("[<64;10;5M"), "");
        assert_eq!(sanitize_name_input("12;3m"), "");
        assert_eq!(sanitize_name_input("[M ab"), "");
        assert_eq!(sanitize_name_input("héllo*wörld!"), "hllowrld");
        assert_eq!(sanitize_name_input("a\tb"), "ab");
    }

    #[test]
    fn task_titles_become_session_titles() {
        assert_eq!(session_title_from_task("  Fix   the bug!  "), "Fix the bug");
        let long = "word ".repeat(20);
        let title = session_title_from_task(&long);
        assert!(title.len() <= 64 && !title.ends_with(' '));
        // The first 64 characters end in a whole word, but TS still cuts at the last space before it.
        assert_eq!(title, "word ".repeat(12).trim());
        let unbroken = "x".repeat(70);
        assert_eq!(session_title_from_task(&unbroken), "x".repeat(64));
    }

    #[test]
    fn base_options_follow_the_branch_setting() {
        assert_eq!(base_options(None)[0].label, "loading branches…");
        let list: BranchList = serde_json::from_value(serde_json::json!({
            "branches": ["main", "feature"], "current": "feature", "defaultBranch": "main", "branchFrom": "current"
        }))
        .unwrap_or_else(|error| panic!("{error}"));
        let options = base_options(Some(&list));
        assert_eq!(options[0].label, "current checkout (feature)");
        assert_eq!(options[0].value, None);
        assert_eq!(options[2].value.as_deref(), Some("feature"));
        let origin = BranchList {
            branch_from: BranchFrom::Origin,
            default_branch: None,
            ..list.clone()
        };
        assert_eq!(base_options(Some(&origin))[0].label, "origin/main (fetched first)");
        let default = BranchList {
            branch_from: BranchFrom::Default,
            default_branch: None,
            ..list.clone()
        };
        assert_eq!(
            base_options(Some(&default))[0].label,
            "the default branch (default branch)"
        );
        let hook = BranchList {
            hook: Some(true),
            ..list
        };
        assert_eq!(base_options(Some(&hook)).len(), 1);
    }

    #[test]
    fn connection_labels_and_parent_workspace() {
        assert_eq!(describe_connection(true), "ready");
        assert_eq!(describe_connection(false), "connecting…");
        assert_eq!(connection_color(true), THEME.success);
        assert_eq!(connection_color(false), THEME.warn);
        let parent = session("p", "Parent", SessionStatus::Running);
        assert_eq!(
            parent_workspace_label(Some(&parent), 40).as_deref(),
            Some("parent dir: /repo")
        );
        assert_eq!(parent_workspace_label(None, 40), None);
        assert!(supports_forked_sub_session(Some(&parent)));
        assert!(!supports_forked_sub_session(None));
        assert!(!fork_stays_in_parent(
            Some(ProgramKey::Claude),
            Some(SubSessionKind::Clean)
        ));
        assert!(!fork_stays_in_parent(None, Some(SubSessionKind::Forked)));
    }

    #[test]
    fn upsert_replaces_and_sorts() {
        let a = session("a", "A", SessionStatus::Running);
        let b = session("b", "B", SessionStatus::Running);
        let mut b2 = b.clone();
        b2.title = "B2".to_owned();
        let next = upsert_session(vec![a.clone(), b], b2);
        assert_eq!(next.len(), 2);
        assert!(next.iter().any(|session| session.title == "B2"));
        assert_eq!(sort_sessions(next.clone()).len(), 2);
    }

    #[test]
    fn derived_values_follow_scope_filter_and_selection() {
        let (mut app, _rx) = app(AppProps::default());
        let mut other = session("o", "Other checkout", SessionStatus::Running);
        other.repo_root = "/elsewhere".to_owned();
        other.cwd = "/elsewhere".to_owned();
        let mut archived = session("z", "Archived", SessionStatus::Exited);
        archived.archived_at = Some("2026-01-01T00:00:00.000Z".to_owned());
        app.data.repo_sessions = vec![session("a", "Alpha", SessionStatus::Running), archived, other];
        assert_eq!(app.sessions().len(), 2);
        let visible: Vec<String> = app.visible_sessions().into_iter().map(|session| session.id).collect();
        assert_eq!(visible, ["a"]);
        assert_eq!(app.selected_session(), None);
        app.set_selected_id(Some("z".to_owned()));
        // Hidden by the active filter: kept, but not actionable.
        assert_eq!(app.selected_session(), None);
        assert_eq!(app.selected_index(), 0);
        app.set_selected_id(Some("a".to_owned()));
        assert_eq!(app.selected_session().map(|session| session.id), Some("a".to_owned()));
        assert_eq!(app.active_attach_target(), AttachTarget::Agent);
        assert!(app.active_pane_ready_for_attach());
        app.set_active_tab(RightPaneTab::Git);
        assert_eq!(app.active_attach_target(), AttachTarget::Git);
        assert!(!app.active_pane_ready_for_attach());
        assert_eq!(app.selected_workspace().as_deref(), Some("/repo"));
        assert!(!app.selected_can_delete_worktree());
        assert!(!app.selected_can_delete_branch());
        assert_eq!(app.spinner_frame(), "⠋");
    }

    #[test]
    fn going_to_a_session_reveals_it() {
        let (mut app, _rx) = app(AppProps::default());
        let parent = session("p", "Parent", SessionStatus::Running);
        let mut child = session("c", "Child", SessionStatus::Running);
        child.parent_session_id = Some("p".to_owned());
        let mut archived = session("z", "Archived", SessionStatus::Exited);
        archived.archived_at = Some("2026-01-01T00:00:00.000Z".to_owned());
        app.data.repo_sessions = vec![parent, child, archived];
        app.browse.collapsed_session_ids.insert("p".to_owned());
        app.browse.session_query = "zzz".to_owned();
        app.go_to_session("c");
        assert_eq!(app.browse.session_query, "");
        assert!(app.browse.collapsed_session_ids.is_empty());
        assert_eq!(app.browse.selected_id.as_deref(), Some("c"));
        assert_eq!(app.mode, Mode::Browse);
        app.go_to_session("z");
        assert_eq!(app.browse.session_filter, SessionFilter::All);
        // A session of another checkout is only reported.
        let mut far = session("f", "Far", SessionStatus::Running);
        far.repo_root = "/other".to_owned();
        far.cwd = "/other".to_owned();
        app.data.repo_sessions.push(far);
        app.go_to_session("f");
        assert_eq!(app.browse.selected_id.as_deref(), Some("z"));
        assert_eq!(
            app.messages.status_message.as_deref(),
            Some("Far is in the sidebar of the Deckhand opened in /other")
        );
        app.open_note("c");
        assert_eq!(app.browse.active_tab, RightPaneTab::Notes);
        assert_eq!(app.browse.session_tabs.get("c"), Some(&RightPaneTab::Notes));
    }

    #[test]
    fn exit_keeps_the_first_result_and_attach_names_the_session() {
        let (mut app, _rx) = app(AppProps::default());
        let running = session("a", "Alpha", SessionStatus::Running);
        app.data.repo_sessions = vec![running.clone()];
        app.attach_to(&running, AttachTarget::Terminal);
        app.quit();
        assert_eq!(
            UiModel::exit(&mut app),
            Some(UiExitResult::Attach {
                session_id: "a".to_owned(),
                target: AttachTarget::Terminal,
                title: Some("Alpha".to_owned()),
                cwd: Some("/repo".to_owned()),
                program: Some(ProgramKey::Claude),
            })
        );
        assert_eq!(UiModel::exit(&mut app), None);
    }

    #[test]
    fn busy_ignores_keys_but_the_details_modes_scroll() {
        let (mut app, _rx) = app(AppProps::default());
        app.set_busy(true);
        app.handle_input(&Input::text("q"));
        assert_eq!(UiModel::exit(&mut app), None);
        app.set_busy(false);
        app.set_mode(Mode::ReviewProject);
        let content = app.details_content().unwrap_or_default();
        assert_eq!(content.title, "Review repository configuration");
        assert_eq!(content.text, "No repository configuration loaded.");
        assert_eq!(content.footer, vec![HintPart::from("esc close")]);
        assert!(!app.scroll_details_pane(&Input::text("x")));
        assert!(app.scroll_details_pane(&Input::text("j")));
        app.set_mode(Mode::ConfirmLoss);
        app.kill.confirmation_draft = "DEL".to_owned();
        let content = app.details_content().unwrap_or_default();
        assert_eq!(content.footer[0].text, "Type DELETE then enter: DEL");
        app.set_mode(Mode::WorkspaceInfo);
        let content = app.details_content().unwrap_or_default();
        assert_eq!(content.title, "Workspace overview");
        assert_eq!(content.text, "Loading workspace information…");
        app.set_mode(Mode::Browse);
        assert_eq!(app.details_content(), None);
    }

    #[test]
    fn flow_effects_apply_to_the_app() {
        let (mut app, _rx) = app(AppProps::default());
        app.data.repo_sessions = vec![session("a", "Alpha", SessionStatus::Running)];
        app.set_selected_id(Some("a".to_owned()));
        app.set_mode(Mode::Tasks);
        let mut changes = deckhand_core::changes_model::empty_changes();
        changes.session_id = Some("a".to_owned());
        changes.branch = Some("main".to_owned());
        let mut stale = changes.clone();
        stale.session_id = Some("b".to_owned());
        let mut updated = session("a", "Renamed", SessionStatus::Running);
        updated.notes = Some("n".to_owned());
        app.apply_flow_effects(vec![
            FlowEffect::SetBusy(true),
            FlowEffect::SetError(Some("boom".to_owned())),
            FlowEffect::SetStatus(Some("ok".to_owned())),
            FlowEffect::Changes(Box::new(changes.clone())),
            FlowEffect::Changes(Box::new(stale)),
            FlowEffect::Session(Box::new(updated)),
            FlowEffect::TasksDoc(Box::new(TasksDoc {
                key: "k".to_owned(),
                path: "/t".to_owned(),
                text: "- [ ] one\n".to_owned(),
                revision: "r".to_owned(),
                too_large: None,
            })),
            FlowEffect::Exit,
        ]);
        assert!(app.messages.busy);
        assert_eq!(app.messages.error.as_deref(), Some("boom"));
        assert_eq!(app.messages.status_message.as_deref(), Some("ok"));
        assert_eq!(app.panes.changes, changes);
        assert_eq!(app.data.repo_sessions[0].title, "Renamed");
        assert_eq!(app.data.tasks.len(), 1);
        assert_eq!(app.mode, Mode::Browse);
        app.apply_flow_effects(vec![FlowEffect::AttachGit]);
        assert!(matches!(
            UiModel::exit(&mut app),
            Some(UiExitResult::Attach {
                target: AttachTarget::Git,
                ..
            })
        ));
    }

    #[test]
    fn resize_falls_back_to_80_by_24() {
        let (mut app, _rx) = app(AppProps::default());
        app.on_resize(100, 0);
        assert_eq!(app.browse.terminal_size.cols, 100);
        assert_eq!(app.browse.terminal_size.rows, 24);
    }
}
