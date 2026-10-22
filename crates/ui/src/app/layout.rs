//! Geometry and frame: the sidebar width rules, the `layout` memo (pane sizes), the header (title, task count, update
//! hint, connection), the right pane per mode and the two-row footer (key hint and messages).
//!
//! The frame is drawn into a canvas of the layout's full size (header, content, footer) and copied onto the
//! terminal, so a terminal smaller than the layout clips it as Ink's overflow would, and no pane draws outside a
//! buffer.

use std::collections::HashSet;
use std::sync::LazyLock;

use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;
use regex::Regex;

use deckhand_core::agent_versions::{installed_versions, update_hint};
use deckhand_core::display::{HINT_SEPARATOR, HintPart, compact_path, fit_hint, truncate};
use deckhand_core::session_features::{TaskTitleOf, session_filter_counts};
use deckhand_core::sidebar_model::{DetailTask, RowRole, SidebarRowsInput, sidebar_rows};
use deckhand_core::tasks_board::{
    linked_task, linked_tasks, note_steps, other_open_tasks, step_count, task_count_label, work_key_of, work_note,
    worktree_open_items,
};
use deckhand_core::workspace::workspace_key;
use deckhand_protocol::data::{RightPaneTab, SessionRecord, SessionStatus};

use crate::app::modes::action_picker::{ActionPickerPaneProps, DevRow, render_action_picker_pane};
use crate::app::modes::confirm::{
    KillConfirmPaneProps, KillOptionKind, MergeConfirmPaneProps, RemoveConfirmPaneProps, kill_loading, kill_options,
    render_conflict_pane, render_kill_dialog, render_merge_pane, render_remove_dialog,
};
use crate::app::modes::create::{
    BaseShown, CreatePaneProps, CreateStep, WorktreePickerProps, filtered_worktrees, render_create_dialog,
    render_worktree_picker,
};
use crate::app::state::{App, DialogState, Mode, TerminalSize};
use crate::app::{
    Derived, base_options, connection_color, describe_connection, format_scroll_sensitivity, hint_short, notes_links,
    parent_workspace_label, supports_forked_sub_session, truthy, with_props,
};
use crate::flow::Flow;
use crate::flows::notes::NotesFlowProps;
use crate::flows::settings::is_settings_flow_mode;
use crate::flows::tasks::{TaskBannerProps, TasksFlowProps, render_task_banner, task_banner_rows};
use crate::flows::worktrees::WorktreesFlowProps;
use crate::panes::details::{DetailsPaneProps, render_details_pane};
use crate::panes::dev::{DevPaneProps, render_dev_pane};
use crate::panes::preview::{PreviewPaneProps, render_preview_pane};
use crate::panes::sidebar::{FilterMenu, SidebarProps, render_sidebar};
use crate::panes::tabs::{TabBadges, open_note_count, render_tab_bar};
use crate::panes::terminal::{TerminalPaneProps, action_status, has_action, render_terminal_pane};
use crate::primitives::{
    Border, Row, Seg, TextStyle, Wrap, bordered, clear_area, dim_area, draw_rows, row_text, row_width, space_between,
    string_width, truncate_row,
};
use crate::theme::{THEME, apply_dusk};

/// TS: app.tsx HEADER_ROWS.
pub const HEADER_ROWS: u16 = 2;

/// TS: app.tsx FOOTER_ROWS: the key hint and one combined message line, always.
pub const FOOTER_ROWS: u16 = 2;

/// TS: app.tsx sidebarWidth: 24% of the width, 24..34, clamped.
pub fn sidebar_width(total_width: i64) -> i64 {
    if total_width <= 0 {
        return 24;
    }
    let width = ((total_width as f64) * 0.24).floor() as i64;
    clamp_sidebar_width(width.clamp(24, 34), total_width)
}

/// Not in the TS: the share of the terminal a fitted sidebar takes at most, in percent.
pub const SIDEBAR_PERCENT: f64 = 30.0;

/// Not in the TS: the bounds of `sidebar_width_percent`, and the step `<`/`>` change it by.
pub const SIDEBAR_PERCENT_MIN: f64 = 15.0;
pub const SIDEBAR_PERCENT_MAX: f64 = 50.0;
pub const SIDEBAR_PERCENT_STEP: f64 = 2.0;

/// Not in the TS: the narrowest sidebar.
pub const SIDEBAR_MIN: i64 = 24;

/// Not in the TS: `sidebar_width_percent` as stored, within its bounds (the default when unset or not a number).
pub fn normalize_sidebar_percent(value: Option<f64>) -> f64 {
    match value {
        Some(value) if value.is_finite() => value.round().clamp(SIDEBAR_PERCENT_MIN, SIDEBAR_PERCENT_MAX),
        _ => SIDEBAR_PERCENT,
    }
}

/// Not in the TS: the sidebar's width: the share you chose (`chosen`, exactly), else fitted to the sessions: at most
/// [`SIDEBAR_PERCENT`], less when every row (`content`, the widest) fits in less.
pub fn sidebar_columns(chosen: Option<f64>, content: Option<i64>, total_width: i64) -> i64 {
    match chosen {
        Some(percent) => proportional_sidebar_width(percent, None, total_width),
        None => proportional_sidebar_width(SIDEBAR_PERCENT, content, total_width),
    }
}

/// Not in the TS: `percent` of the terminal, or less when every row (`content`, the widest row's whole width) fits in
/// less, never under [`SIDEBAR_MIN`]. It never cuts a title just to save room.
pub fn proportional_sidebar_width(percent: f64, content: Option<i64>, total_width: i64) -> i64 {
    let share = ((total_width as f64) * percent / 100.0).floor() as i64;
    let width = content.map_or(share, |content| share.min(content));
    clamp_sidebar_width(width.max(SIDEBAR_MIN), total_width)
}

/// Not in the TS: the whole width (border and padding included) the sidebar needs to show each of `rows` uncut.
pub fn sidebar_row_widths(input: &SidebarRowsInput<'_>) -> Vec<i64> {
    // Laid out wide: every title whole, then measured without the gap that pads it out.
    let wide = SidebarRowsInput { width: 1000, ..*input };
    sidebar_rows(&wide)
        .iter()
        .map(|row| {
            let total: usize = row.parts.iter().map(|part| string_width(&part.text)).sum();
            // The widest gap pads the title out to the markers; the one-column gaps between markers stay.
            let padding = row
                .parts
                .iter()
                .filter(|part| part.role == RowRole::Gap)
                .map(|part| string_width(&part.text))
                .max()
                .unwrap_or(0);
            let content = total.saturating_sub(padding);
            // One column between the title and its markers, then the border and padding (row = width - 3).
            i64::try_from(content).unwrap_or(i64::MAX).saturating_add(1 + 3)
        })
        .collect()
}

/// TS: app.tsx clampSidebarWidth.
pub fn clamp_sidebar_width(width: i64, total_width: i64) -> i64 {
    let min_width = 18.min(10.max(total_width - 23));
    let half = ((total_width as f64) * 0.5).floor() as i64;
    let max_width = min_width.max(half.min(total_width - 23));
    min_width.max(max_width.min(width))
}

/// TS: app.tsx `layout` memo.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AppLayout {
    pub sidebar_width: u16,
    pub preview_width: u16,
    pub content_height: u16,
    /// The right pane minus border and padding (4 columns).
    pub pane_inner_width: u16,
    /// The right pane minus border, tab bar and spacer (4 rows).
    pub pane_inner_height: u16,
    pub preview_cols: u16,
    pub preview_rows: u16,
}

/// A layout number as a terminal dimension.
fn dimension(value: i64) -> u16 {
    u16::try_from(value.max(0)).unwrap_or(u16::MAX)
}

/// TS: app.tsx `layout`: pane sizes for the terminal size and the sidebar override.
pub fn app_layout(size: TerminalSize, sidebar_width_override: Option<i64>) -> AppLayout {
    let total_width = i64::from(size.cols);
    let total_height = i64::from(size.rows);
    let left = clamp_sidebar_width(
        sidebar_width_override.unwrap_or_else(|| sidebar_width(total_width)),
        total_width,
    );
    let separator_width = 1;
    let right = 20.max(total_width - left - separator_width);
    let content_height = 8.max(total_height - i64::from(HEADER_ROWS) - i64::from(FOOTER_ROWS));
    // The right pane's wrapper takes 4 columns (border 2 + paddingX 2) and 4 rows (border 2 + tab bar 1 + spacer 1)
    // before the sub-pane's content begins.
    let pane_inner_width = 10.max(right - 4);
    let pane_inner_height = 4.max(content_height - 4);
    let preview_rows = 1.max(pane_inner_height - 1);
    AppLayout {
        sidebar_width: dimension(left),
        preview_width: dimension(right),
        content_height: dimension(content_height),
        pane_inner_width: dimension(pane_inner_width),
        pane_inner_height: dimension(pane_inner_height),
        preview_cols: dimension(pane_inner_width),
        preview_rows: dimension(preview_rows),
    }
}

/// TS: footerHint's `terminalAction` argument.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TerminalActionHint {
    pub switch_hint: String,
    pub finished: bool,
}

/// TS: app.tsx footerHint's arguments.
#[derive(Clone, Copy, Debug)]
pub struct FooterHintInput<'a> {
    pub mode: Mode,
    pub active_tab: RightPaneTab,
    pub width: usize,
    pub session: Option<&'a SessionRecord>,
    pub scroll_sensitivity: f64,
    pub attach_ready: bool,
    pub notes_hint: Option<&'a str>,
    pub terminal_action: Option<&'a TerminalActionHint>,
    /// Not in the TS: the Dev tab's process switch (`v next: <name>`), with named processes.
    pub dev_switch: Option<&'a str>,
}

/// A hint part dropped at level `drop` (TS `{text, drop}`).
fn dropped(text: &str, drop: i32) -> HintPart {
    HintPart {
        text: text.to_owned(),
        short: None,
        drop: Some(drop),
    }
}

/// A hint part with a short form, dropped at level `drop` (TS `{text, short, drop}`).
fn dropped_short(text: &str, short: &str, drop: i32) -> HintPart {
    HintPart {
        text: text.to_owned(),
        short: Some(short.to_owned()),
        drop: Some(drop),
    }
}

/// TS: app.tsx footerHint: the footer's key hint per mode (empty where the screen shows its own).
pub fn footer_hint(input: &FooterHintInput<'_>) -> String {
    let width = input.width;
    match input.mode {
        // Every other mode draws a screen (or a dialog over the right pane) that shows its own (single) hint line;
        // Search, Tasks and Worktrees write their own line (the footer).
        Mode::Help
        | Mode::Settings
        | Mode::Agents
        | Mode::EditProject
        | Mode::DiscardProject
        | Mode::WorkspaceInfo
        | Mode::ReviewProject
        | Mode::ConfirmLoss
        | Mode::PickAction
        | Mode::PickProgram
        | Mode::EnterName
        | Mode::PickWorktree
        | Mode::ConfirmKill
        | Mode::ConfirmMerge
        | Mode::MergeConflicts
        | Mode::Tasks
        | Mode::ConfirmRemove
        | Mode::Worktrees
        | Mode::Search => String::new(),
        Mode::PreviewFocus => fit_hint(
            &[
                hint_short("j/k ↑↓ J/K scroll", "j/k scroll"),
                dropped("g/G top/bottom", 1),
                dropped_short(
                    &format!(
                        "wheel ×{} (+/− speed)",
                        format_scroll_sensitivity(input.scroll_sensitivity)
                    ),
                    "+/− wheel speed",
                    2,
                ),
                "← back".into(),
            ],
            width,
            HINT_SEPARATOR,
        ),
        Mode::ChangesFocus => fit_hint(
            &[
                hint_short("space stage/unstage", "space stage"),
                dropped_short("a/A stage/unstage all", "a/A all", 1),
                hint_short("enter/E open in editor", "enter editor"),
                dropped("J/K scroll diff", 2),
                dropped("o lazygit", 1),
                "← back".into(),
            ],
            width,
            HINT_SEPARATOR,
        ),
        Mode::NotesFocus => input.notes_hint.unwrap_or("esc done").to_owned(),
        Mode::PickFilter => fit_hint(
            &[
                hint_short("a r ! e A * pick", "a r ! e A *"),
                dropped_short("f back to active", "f active", 1),
                "enter keep".into(),
                "esc cancel".into(),
            ],
            width,
            HINT_SEPARATOR,
        ),
        Mode::Browse => fit_hint(&browse_hint_parts(input), width, HINT_SEPARATOR),
    }
}

/// TS: app.tsx FooterMessage.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FooterMessage {
    pub text: String,
    pub color: Color,
}

/// TS: app.tsx candidateMessages: error, busy, numeric selection, status and the selected session's cleanup error.
pub fn footer_messages(app: &App) -> Vec<FooterMessage> {
    footer_messages_for(app, app.selected_session().as_ref())
}

fn footer_messages_for(app: &App, selected: Option<&SessionRecord>) -> Vec<FooterMessage> {
    let message = |text: String, color: Color| FooterMessage { text, color };
    let mut messages = Vec::new();
    if let Some(error) = app.messages.error.as_ref().filter(|error| !error.is_empty()) {
        messages.push(message(format!("Error: {error}"), THEME.error));
    }
    if app.messages.busy {
        messages.push(message("Working…".to_owned(), THEME.warn));
    }
    if !app.browse.numeric_selection.is_empty() {
        messages.push(message(
            format!("Select session: {}", app.browse.numeric_selection),
            THEME.active,
        ));
    }
    if let Some(status) = app.messages.status_message.as_ref().filter(|status| !status.is_empty()) {
        messages.push(message(status.clone(), THEME.success));
    }
    if let Some(cleanup) = selected
        .and_then(|session| session.cleanup_error.as_ref())
        .filter(|text| !text.is_empty())
    {
        messages.push(message(cleanup.clone(), THEME.error));
    }
    messages
}

static LINE_BREAK_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\s*\n\s*").unwrap_or_else(|_| unreachable!()));

/// TS: app.tsx footerRows: exactly two rows, each truncated, so the layout above never shifts.
pub fn footer_rows(app: &App) -> [Row; 2] {
    let derived = app.derived();
    footer_rows_for(app, &derived)
}

/// The Tasks / Worktrees / Notes flows' footer hints (their props from the app, borrowed).
fn flow_hint(app: &App, derived: &Derived, width: usize) -> Option<String> {
    let repo_refs: Vec<&SessionRecord> = app.data.repo_sessions.iter().collect();
    match app.mode {
        Mode::Tasks => {
            let props = TasksFlowProps {
                client: app.connection.client.as_ref(),
                repo_root: &app.props.repo_root,
                doc: app.data.tasks_doc.as_ref(),
                tasks: &app.data.tasks,
                sessions: &repo_refs,
                spinner_frame: app.spinner_frame(),
            };
            Some(app.flows.tasks.hint(&props, width))
        }
        Mode::Worktrees => {
            let props = WorktreesFlowProps {
                client: app.connection.client.as_ref(),
                cwd: &app.props.repo_root,
                sessions: &repo_refs,
                spinner_frame: app.spinner_frame(),
            };
            Some(app.flows.worktrees.hint(&props, width))
        }
        Mode::NotesFocus => {
            let refs: Vec<&SessionRecord> = derived.sessions.iter().collect();
            let lookup = notes_links(app);
            let props = NotesFlowProps {
                client: app.connection.client.as_ref(),
                session: derived.selected.as_ref(),
                sessions: &refs,
                focused: true,
                links: lookup.as_deref(),
            };
            Some(app.flows.notes.hint(&props, width))
        }
        _ => None,
    }
}

fn footer_rows_for(app: &App, derived: &Derived) -> [Row; 2] {
    let cols = usize::from(app.browse.terminal_size.cols);
    let selected = derived.selected.as_ref();
    let (hint, color) = match app.mode {
        Mode::Search => (
            format!("Search: /{} • enter keep • esc clear", app.browse.session_query),
            THEME.active,
        ),
        Mode::Tasks | Mode::Worktrees => (flow_hint(app, derived, cols).unwrap_or_default(), THEME.hint),
        mode => {
            let action = &app.panes.action;
            let showing = app.showing_action();
            let dev_switch = app.next_dev_process().map(|next| format!("v {next}"));
            let terminal_action = has_action(selected, action).then(|| TerminalActionHint {
                switch_hint: if showing {
                    "v shell".to_owned()
                } else {
                    format!(
                        "v {} {}",
                        action.name.as_deref().unwrap_or("action"),
                        action_status(action).text
                    )
                },
                finished: showing && !action.live,
            });
            let notes_hint = if mode == Mode::NotesFocus {
                flow_hint(app, derived, cols)
            } else {
                None
            };
            let text = footer_hint(&FooterHintInput {
                mode,
                active_tab: app.browse.active_tab,
                width: cols,
                session: selected,
                scroll_sensitivity: app.preview_focus.scroll_sensitivity,
                attach_ready: app.active_pane_ready_for_attach(),
                notes_hint: notes_hint.as_deref(),
                terminal_action: terminal_action.as_ref(),
                dev_switch: dev_switch.as_deref(),
            });
            (text, THEME.hint)
        }
    };
    let hint_row = truncate_row(&[Seg::new(hint, TextStyle::fg(color))], cols, Wrap::TruncateEnd);
    let messages = footer_messages_for(app, selected);
    let message_row = if messages.is_empty() {
        vec![Seg::plain(" ")]
    } else {
        let segs: Row = messages
            .iter()
            .enumerate()
            .map(|(index, message)| {
                let separator = if index > 0 { " · " } else { "" };
                let text = LINE_BREAK_PATTERN.replace_all(&message.text, " ");
                Seg::new(format!("{separator}{text}"), TextStyle::fg(message.color))
            })
            .collect();
        truncate_row(&segs, cols, Wrap::TruncateEnd)
    };
    [hint_row, message_row]
}

/// TS: `<Box justifyContent="space-between">` of the title and the `wrap="truncate-start"` status: both shrink in
/// proportion to their width (Yoga's flex-shrink) when they do not fit.
fn header_space_between(left: &[Seg], right: &[Seg], width: usize) -> Row {
    let left_width = row_width(left);
    let right_width = row_width(right);
    if left_width + right_width <= width {
        return space_between(left, right, width);
    }
    let overflow = (left_width + right_width - width) as f64;
    let total = (left_width + right_width) as f64;
    let left_shrunk = ((left_width as f64) - overflow * (left_width as f64) / total).round() as usize;
    let left_room = left_shrunk.min(width);
    let mut row = truncate_row(left, left_room, Wrap::TruncateEnd);
    let used = row_width(&row);
    row.extend(truncate_row(right, width.saturating_sub(used), Wrap::TruncateStart));
    row
}

/// TS: app.tsx the header: the title row (`deckhand` / DEV, task count, update hint, connection) and the repo path.
pub fn header_rows(app: &App) -> [Row; 2] {
    let cols = usize::from(app.browse.terminal_size.cols);
    let muted = TextStyle::fg(THEME.muted);
    let title = if deckhand_sys::paths::is_dev_channel() {
        "deckhand · DEV (isolated)"
    } else {
        "deckhand"
    };
    let left = vec![Seg::new(title, TextStyle::fg(THEME.accent).bold(true))];
    // Open tasks in the header, so a forgotten backlog stays in sight (the first thing to go when narrow).
    let task_count = if cols >= 70 {
        task_count_label(&app.data.tasks)
    } else {
        String::new()
    };
    // Muted, only when an installed agent has a newer release.
    let agent_update_hint = update_hint(app.data.agent_versions.as_ref());
    let connected = app.connection.client.is_some();
    let mut right = Vec::new();
    if !task_count.is_empty() {
        right.push(Seg::new(format!("{task_count}   "), muted));
    }
    if !agent_update_hint.is_empty() {
        right.push(Seg::new(format!("{agent_update_hint}   "), muted));
    }
    right.push(Seg::new(
        format!("● {}", describe_connection(connected)),
        TextStyle::fg(connection_color(connected)),
    ));
    // The sidebar header shows the filter, search and counts, so the repo path gets the whole row.
    let repo_label_width = cols.max(1);
    let repo = truncate(&compact_path(&app.props.repo_root, repo_label_width), repo_label_width);
    [
        header_space_between(&left, &right, cols),
        truncate_row(&[Seg::new(repo, muted)], cols, Wrap::TruncateEnd),
    ]
}

/// Not in the TS: measures the widest listed row as the sidebar draws it (its markers, `+N` counts and `↑` included),
/// for the sidebar to shrink to when every row fits in less than its share.
pub fn measure_sidebar(app: &mut App, derived: &Derived) {
    let visible: Vec<&SessionRecord> = derived.visible.iter().collect();
    let all: Vec<&SessionRecord> = derived.sessions.iter().collect();
    let none = HashSet::new();
    let applied = app.collapse_applied();
    let collapsed: HashSet<String> = app.browse.collapsed_session_ids.iter().cloned().collect();
    let hidden: HashSet<String> = app.browse.hidden_exited_session_ids.iter().cloned().collect();
    let versions = installed_versions(app.data.agent_versions.as_ref());
    let widths = sidebar_row_widths(&SidebarRowsInput {
        rows: &visible,
        all_sessions: &all,
        first_number: 1,
        number_width: visible.len().max(1).to_string().len(),
        selected_id: None,
        width: 0,
        spinner_frame: app.spinner_frame(),
        filter: app.browse.session_filter,
        collapsed_session_ids: Some(if applied { &collapsed } else { &none }),
        hidden_session_ids: Some(if applied { &hidden } else { &none }),
        installed_versions: Some(&versions),
        shared_dev: true,
    });
    app.browse.sidebar_content = widths.into_iter().max();
}

/// The sidebar: the visible sessions, the selection's details and the filter menu while it is open.
fn render_sidebar_for(app: &App, derived: &Derived, buf: &mut Buffer, area: Rect) {
    let visible: Vec<&SessionRecord> = derived.visible.iter().collect();
    let all: Vec<&SessionRecord> = derived.sessions.iter().collect();
    let none = HashSet::new();
    let collapse_applied = app.collapse_applied();
    let collapsed: HashSet<String> = app.browse.collapsed_session_ids.iter().cloned().collect();
    let hidden: HashSet<String> = app.browse.hidden_exited_session_ids.iter().cloned().collect();
    let versions = installed_versions(app.data.agent_versions.as_ref());
    let tasks = &app.data.tasks;
    let task_of: &dyn Fn(&SessionRecord) -> Option<DetailTask> = &|session: &SessionRecord| {
        linked_task(tasks, Some(session)).map(|task| DetailTask {
            title: task.title.clone(),
            done: task.done,
            more: Some(other_open_tasks(tasks, Some(session))),
        })
    };
    // Only while the filter menu is open: how many rows each filter would list.
    let filter_menu = (app.mode == Mode::PickFilter).then(|| {
        let title = |session: &SessionRecord| {
            Some(
                linked_tasks(tasks, Some(session))
                    .iter()
                    .map(|task| task.title.as_str())
                    .collect::<Vec<_>>()
                    .join(" "),
            )
        };
        let title: TaskTitleOf<'_> = &title;
        FilterMenu {
            selected: app.browse.session_filter,
            counts: session_filter_counts(&derived.sessions, &app.browse.session_query, Some(title)),
        }
    });
    let props = SidebarProps {
        sessions: &visible,
        all_sessions: &all,
        selected_id: derived.selected.as_ref().map(|session| session.id.as_str()),
        spinner_frame: app.spinner_frame(),
        collapsed_session_ids: Some(if collapse_applied { &collapsed } else { &none }),
        hidden_session_ids: Some(if collapse_applied { &hidden } else { &none }),
        loaded: app.data.sessions_loaded,
        filter: app.browse.session_filter,
        query: &app.browse.session_query,
        now: deckhand_core::jsstr::date::now_ms(),
        installed_versions: Some(&versions),
        task_of: Some(task_of),
        filter_menu,
    };
    render_sidebar(&props, buf, area);
}

/// TS: app.tsx the right pane in the tabbed modes: the bordered box with the tab bar and the active tab.
pub fn render_tabbed_pane(app: &mut App, buf: &mut Buffer, area: Rect) {
    let derived = app.derived();
    render_tabbed_pane_for(app, &derived, buf, area);
}

fn render_tabbed_pane_for(app: &mut App, derived: &Derived, buf: &mut Buffer, area: Rect) {
    let layout = app.layout();
    let selected = derived.selected.as_ref();
    let inner = bordered(buf, area, &Border::round(THEME.border), 1);
    let badges = TabBadges {
        dev_running: selected.is_some_and(|session| session.dev_running == Some(true)),
        open_notes: selected.map_or(0, open_note_count),
    };
    render_tab_bar(
        app.browse.active_tab,
        badges,
        buf,
        Rect::new(inner.x, inner.y, layout.pane_inner_width, 1),
    );
    let pane = Rect::new(
        inner.x,
        inner.y.saturating_add(2),
        layout.pane_inner_width,
        layout.pane_inner_height,
    );
    match app.browse.active_tab {
        RightPaneTab::Preview => {
            let props = PreviewPaneProps {
                session: selected,
                preview: &app.panes.preview,
                spinner_frame: app.spinner_frame(),
                focused: app.mode == Mode::PreviewFocus,
            };
            render_preview_pane(&props, buf, pane);
        }
        RightPaneTab::Terminal => {
            let props = TerminalPaneProps {
                session: selected,
                terminal: &app.panes.terminal,
                action: &app.panes.action,
                view: app.panes.terminal_view,
            };
            render_terminal_pane(&props, buf, pane);
        }
        RightPaneTab::Git => {
            with_props!(
                changes,
                app,
                |flow: &mut crate::flows::changes::ChangesFlow, props: &crate::flows::changes::ChangesFlowProps<'_>| {
                    flow.render(props, buf, pane)
                }
            );
        }
        RightPaneTab::Dev => {
            let props = DevPaneProps {
                session: selected,
                dev: &app.panes.dev,
                selected_process: app.panes.dev_process.as_deref(),
            };
            render_dev_pane(&props, buf, pane);
        }
        RightPaneTab::Notes => {
            let task = linked_task(&app.data.tasks, selected).cloned();
            let mut notes_area = pane;
            if let (Some(task), Some(session)) = (task, selected) {
                let repo_refs: Vec<&SessionRecord> = app.data.repo_sessions.iter().collect();
                let note = work_note(&work_key_of(session), &repo_refs);
                let props = TaskBannerProps {
                    task: &task,
                    more: other_open_tasks(&app.data.tasks, Some(session)),
                    steps: Some(step_count(&note_steps(note.as_ref().map(|note| note.text.as_str())))),
                    sessions: &repo_refs,
                    spinner_frame: app.spinner_frame(),
                };
                let banner_rows = task_banner_rows(&props, usize::from(pane.width)).len();
                let banner_height = u16::try_from(banner_rows).unwrap_or(u16::MAX);
                render_task_banner(&props, buf, Rect::new(pane.x, pane.y, pane.width, banner_height));
                notes_area = Rect::new(
                    pane.x,
                    pane.y.saturating_add(banner_height),
                    pane.width,
                    layout.pane_inner_height.saturating_sub(2).max(1),
                );
            }
            with_props!(
                notes,
                app,
                |flow: &mut crate::flows::notes::NotesFlow, props: &NotesFlowProps<'_>| {
                    flow.render(props, buf, notes_area)
                }
            );
        }
    }
}

/// TS: app.tsx the content row per mode: full-width screens (settings, help, agents), or the sidebar beside the
/// mode's right pane.
pub fn render_content(app: &mut App, buf: &mut Buffer, area: Rect) {
    let derived = app.derived();
    render_content_for(app, &derived, buf, area);
}

fn render_content_for(app: &mut App, derived: &Derived, buf: &mut Buffer, area: Rect) {
    let layout = app.layout();
    let mode = app.mode;
    // Settings take the full width: the grid shows both layers side by side.
    if is_settings_flow_mode(mode) {
        with_props!(settings, app, |flow: &mut crate::flows::settings::SettingsFlow,
                                    props: &crate::flows::settings::SettingsFlowProps<
            '_,
        >| { flow.render(props, buf, area) });
        return;
    }
    if mode == Mode::Help {
        app.flows.help.render(buf, area);
        return;
    }
    if mode == Mode::Agents {
        with_props!(
            agents,
            app,
            |flow: &mut crate::flows::agents::AgentsFlow, props: &crate::flows::agents::AgentsFlowProps<'_>| {
                flow.render(props, buf, area)
            }
        );
        return;
    }
    let sidebar = Rect::new(area.x, area.y, layout.sidebar_width, layout.content_height);
    render_sidebar_for(app, derived, buf, sidebar);
    let right = Rect::new(
        area.x.saturating_add(layout.sidebar_width).saturating_add(1),
        area.y,
        layout.preview_width,
        layout.content_height,
    );
    // A dialog keeps the session's pane in sight behind it.
    if mode.is_dialog() {
        render_tabbed_pane_for(app, derived, buf, right);
        dim_area(buf, right);
    }
    // Another dialog (or screen) starts scrolled to the top and at its own size.
    if mode.scrolls_body() {
        // Picking the agent and naming the session are one dialog: it keeps its size from one step to the next.
        let dialog = if mode == Mode::EnterName {
            Mode::PickProgram
        } else {
            mode
        };
        if app.dialog.mode != Some(dialog) {
            app.dialog = DialogState {
                mode: Some(dialog),
                ..DialogState::default()
            };
        }
    }
    let view = app.dialog.last.view(app.dialog.scroll);
    let selected = derived.selected.as_ref();
    let session_refs: Vec<&SessionRecord> = derived.sessions.iter().collect();
    match mode {
        _ if mode.shows_tabs() => render_tabbed_pane_for(app, derived, buf, right),
        Mode::Tasks => {
            with_props!(
                tasks,
                app,
                |flow: &mut crate::flows::tasks::TasksFlow, props: &TasksFlowProps<'_>| {
                    flow.render(props, buf, right)
                }
            );
        }
        Mode::Worktrees => {
            with_props!(
                worktrees,
                app,
                |flow: &mut crate::flows::worktrees::WorktreesFlow, props: &WorktreesFlowProps<'_>| {
                    flow.render(props, buf, right)
                }
            );
        }
        Mode::ConfirmRemove => {
            let props = RemoveConfirmPaneProps {
                session: selected,
                items: &app.remove.items,
            };
            app.dialog.last = render_remove_dialog(&props, buf, right, view);
        }
        Mode::WorkspaceInfo | Mode::ReviewProject | Mode::ConfirmLoss => {
            if let Some(details) = app.details_content() {
                let props = DetailsPaneProps {
                    title: &details.title,
                    text: &details.text,
                    footer: &details.footer,
                    scroll: details.scroll,
                };
                render_details_pane(&props, buf, right);
            }
        }
        Mode::PickAction => {
            let project = app.action_picker.action_project.as_ref();
            let action = &app.panes.action;
            let running =
                (action.session_id == project.and_then(|project| project.session_id.clone())).then_some(action);
            let dev = app.run_list_has_dev().then(|| DevRow {
                running: selected.is_some_and(|session| {
                    session.dev_running == Some(true)
                        || (app.panes.dev.session_id.as_deref() == Some(session.id.as_str()) && app.panes.dev.live)
                }),
            });
            let processes = app.run_list_processes();
            let props = ActionPickerPaneProps {
                project: project.map(|project| &project.project),
                running,
                dev,
                processes: &processes,
                selected_index: app.action_picker.action_index,
            };
            render_action_picker_pane(&props, buf, right);
        }
        Mode::PickWorktree => {
            let worktrees = filtered_worktrees(&app.create.worktrees, &app.create.worktree_query);
            let props = WorktreePickerProps {
                worktrees: &worktrees,
                selected_index: app.create.worktree_index,
                query: &app.create.worktree_query,
                typing: app.create.worktree_typing,
                total_count: app.create.worktrees.len(),
            };
            app.dialog.last = render_worktree_picker(&props, buf, right, view);
        }
        Mode::ConfirmKill => {
            let check = app.current_cleanup_check();
            let options = kill_options(
                app.selected_can_delete_worktree(),
                app.selected_can_delete_branch(),
                check.and_then(|check| check.worktree.as_ref()),
                check.and_then(|check| check.branch.as_ref()),
            );
            let index = app.kill.confirm_index.min(options.len().saturating_sub(1));
            let delete_branch = options
                .get(index)
                .is_some_and(|option| option.kind == KillOptionKind::DeleteBranch);
            let inspection = app.cleanup_inspection_for(delete_branch);
            let worktree_id = selected
                .and_then(|session| session.worktree.as_ref())
                .and_then(|worktree| worktree.id.as_deref());
            let open_items = worktree_open_items(worktree_id, &app.data.repo_sessions);
            let props = KillConfirmPaneProps {
                session: selected,
                sessions: &session_refs,
                options: &options,
                selected_index: index,
                force: app.kill.force,
                inspection,
                open_items: &open_items,
                keep_open_items: app.kill.keep_open_items,
                loading: kill_loading(app),
                other_inspection: app.cleanup_inspection_for(!delete_branch),
            };
            app.dialog.last = render_kill_dialog(&props, buf, right, view);
        }
        Mode::ConfirmMerge => {
            let flow = app
                .merge
                .flow
                .as_ref()
                .filter(|flow| selected.is_some_and(|session| session.id == flow.session_id));
            let props = MergeConfirmPaneProps {
                session: selected,
                sessions: &session_refs,
                flow,
                selected_index: app.merge.confirm_index,
                tasks: &app.data.tasks,
                now: Some(deckhand_core::jsstr::date::now_ms()),
            };
            app.dialog.last = render_merge_pane(&props, buf, right, app.dialog.scroll);
        }
        Mode::MergeConflicts => {
            if let Some(conflict) = app.merge.conflict.as_ref() {
                app.dialog.last = render_conflict_pane(&conflict.result, buf, right, app.dialog.scroll);
            }
        }
        Mode::PickProgram | Mode::EnterName => {
            let create = &app.create;
            let parent = create
                .parent_id
                .as_ref()
                .and_then(|id| derived.sessions.iter().find(|session| &session.id == id));
            let has_parent = create.parent_id.is_some();
            let options = base_options(create.branch_list.as_ref());
            let index = create.base_index.min(options.len().saturating_sub(1));
            let base = BaseShown {
                label: options
                    .get(index)
                    .map(|option| option.label.clone())
                    .unwrap_or_default(),
                index,
                count: options.len(),
            };
            let workspace_label = parent_workspace_label(parent, usize::from(layout.preview_width));
            let props = CreatePaneProps {
                mode: if mode == Mode::PickProgram {
                    CreateStep::PickProgram
                } else {
                    CreateStep::EnterName
                },
                program_index: create.program_index,
                draft_name: &create.draft_name,
                worktree_mode: create.worktree_mode,
                parent_title: parent.map(|parent| parent.title.as_str()),
                parent_workspace_label: workspace_label.as_deref(),
                sub_session_kind: create.sub_session_kind,
                show_fork_option: has_parent && create.handoff_from_id.is_none() && supports_forked_sub_session(parent),
                show_handoff_option: has_parent
                    && create.handoff_from_id.is_none()
                    && parent.is_some_and(|parent| truthy(parent.handoff_path.as_ref())),
                from_handoff: create.handoff_from_id.is_some(),
                task_title: create
                    .task_start
                    .as_ref()
                    .filter(|_| !has_parent)
                    .map(|task| task.title.as_str()),
                base: Some(&base),
            };
            app.dialog.last = render_create_dialog(&props, buf, right, view);
        }
        _ => {}
    }
}

/// The most rows an error toast takes, border included.
const ERROR_TOAST_MAX_ROWS: usize = 8;

/// Not in the TS: an error too long for the footer's message row also shows whole, wrapped (its own line breaks
/// kept), in a box of at most [`ERROR_TOAST_MAX_ROWS`] rows at the top right of the content, as long as the footer
/// shows it (the top, so the preview's latest lines, where an agent asks for input, stay in view). `None` when it
/// fits the row (`cols` wide).
pub fn error_toast_rows(error: &str, cols: usize) -> Option<(usize, Vec<Row>)> {
    let text = format!("Error: {}", error.trim());
    if !text.contains('\n') && string_width(&text) <= cols {
        return None;
    }
    let width = cols.saturating_sub(2).min(cols.max(40) / 2 + 20).max(1);
    let inner = width.saturating_sub(4).max(1);
    let mut rows: Vec<Row> = text
        .lines()
        .flat_map(|line| deckhand_core::display::wrap_words(line.trim_end(), inner))
        .map(|line| vec![Seg::new(line, TextStyle::fg(THEME.error))])
        .collect();
    let room = ERROR_TOAST_MAX_ROWS - 2;
    if rows.len() > room {
        rows.truncate(room);
        if let Some(last) = rows.last_mut() {
            *last = truncate_row(
                &[Seg::new(format!("{}…", row_text(last)), TextStyle::fg(THEME.error))],
                inner,
                Wrap::TruncateEnd,
            );
        }
    }
    Some((width, rows))
}

/// TS: app.tsx App's render: header, content, footer.
pub fn render_app(app: &mut App, frame: &mut Frame<'_>) {
    let derived = app.derived();
    let layout = app.layout();
    let cols = app.browse.terminal_size.cols;
    let footer_y = HEADER_ROWS.saturating_add(layout.content_height);
    let height = footer_y.saturating_add(FOOTER_ROWS);
    let mut canvas = Buffer::empty(Rect::new(0, 0, cols, height));
    draw_rows(&mut canvas, Rect::new(0, 0, cols, HEADER_ROWS), &header_rows(app));
    render_content_for(
        app,
        &derived,
        &mut canvas,
        Rect::new(0, HEADER_ROWS, cols, layout.content_height),
    );
    draw_rows(
        &mut canvas,
        Rect::new(0, footer_y, cols, FOOTER_ROWS),
        &footer_rows_for(app, &derived),
    );
    if let Some((width, rows)) = app
        .messages
        .error
        .as_deref()
        .filter(|error| !error.is_empty())
        .and_then(|error| error_toast_rows(error, usize::from(cols)))
    {
        let width = u16::try_from(width).unwrap_or(cols);
        let height = u16::try_from(rows.len() + 2)
            .unwrap_or(u16::MAX)
            .min(layout.content_height);
        let toast = Rect::new(cols.saturating_sub(width + 1), HEADER_ROWS, width, height);
        clear_area(&mut canvas, toast);
        let content = bordered(&mut canvas, toast, &Border::round(THEME.border_danger), 1);
        draw_rows(&mut canvas, content, &rows);
    }
    let area = frame.area();
    let visible = area.intersection(canvas.area);
    let buf = frame.buffer_mut();
    for y in visible.top()..visible.bottom() {
        for x in visible.left()..visible.right() {
            buf[(x, y)] = canvas[(x, y)].clone();
        }
    }
    apply_dusk(buf, area, &THEME);
}

/// The hint parts of the browse footer (TS footerHint's `browse` case), before fitting.
pub fn browse_hint_parts(input: &FooterHintInput<'_>) -> Vec<HintPart> {
    let session = input.session;
    let tab = input.active_tab;
    let ready = input.attach_ready;
    let running = session.is_some_and(|session| session.status == SessionStatus::Running);
    // Terminal, Git and Dev belong to the session's workspace, so they are usable whether or not the agent runs.
    let has_workspace = session.and_then(workspace_key).is_some_and(|key| !key.is_empty());
    let finished = input.terminal_action.is_some_and(|action| action.finished);
    // → steps into the pane, Enter opens it full screen.
    let attach = match tab {
        RightPaneTab::Dev => ready.then_some("enter open"),
        RightPaneTab::Git => has_workspace.then_some(if ready {
            "→ changes • enter lazygit"
        } else {
            "loading…"
        }),
        // A finished action shown on the Terminal tab cannot be attached (not "loading": it will not become ready).
        RightPaneTab::Terminal => {
            (has_workspace && !finished).then_some(if ready { "enter open" } else { "loading…" })
        }
        RightPaneTab::Preview => running.then_some(if ready {
            "→ scroll • enter open"
        } else {
            "loading…"
        }),
        RightPaneTab::Notes => None,
    };
    let pane: Option<String> = match (tab, session) {
        (RightPaneTab::Notes, Some(_)) => Some("enter edit notes • o vim • E open in editor".to_owned()),
        (RightPaneTab::Dev, Some(session)) if workspace_key(session).is_some_and(|key| !key.is_empty()) => {
            let toggle = if session.dev_running == Some(true) {
                "d d stop Dev"
            } else {
                "d d start Dev"
            };
            Some(match input.dev_switch {
                Some(switch) => format!("{switch} • {toggle}"),
                None => toggle.to_owned(),
            })
        }
        (RightPaneTab::Terminal, _) => input.terminal_action.map(|action| action.switch_hint.clone()),
        _ => None,
    };
    let deleted = session.is_some_and(|session| {
        session
            .worktree
            .as_ref()
            .is_some_and(|worktree| truthy(worktree.deleted_at.as_ref()))
    });
    let lifecycle = match session.map(|session| session.status) {
        Some(SessionStatus::Exited) => (!deleted).then_some("s resume • S fresh"),
        Some(SessionStatus::Running) => Some("x stop"),
        Some(SessionStatus::Starting) => Some("x cancel start"),
        None => None,
    };
    let archived = session.is_some_and(|session| truthy(session.archived_at.as_ref()));
    let exited = session.is_some_and(|session| session.status == SessionStatus::Exited);
    // Archiving is offered where it is the likely next step (finished sessions), and undoing it where it applies.
    let archive = if archived {
        Some(dropped("A unarchive", 2))
    } else if exited {
        Some(dropped("backspace archive", 2))
    } else {
        None
    };
    // Higher drop numbers go first when the line is too narrow; ? help always stays.
    let remove = (exited && archived).then(|| dropped_short("backspace remove for good", "backspace remove", 1));
    // E opens the session's folder (Notes lists its own E, the note); a deleted worktree has no folder.
    let editor = (session.is_some() && tab != RightPaneTab::Notes && !deleted)
        .then(|| dropped_short("E open in editor", "E editor", 2));
    let parts: Vec<Option<HintPart>> = vec![
        attach.map(HintPart::from),
        pane.as_deref().map(HintPart::from),
        lifecycle.map(HintPart::from),
        archive,
        remove,
        editor,
        Some("? help".into()),
        Some(dropped("n new", 1)),
        Some(dropped("W worktrees", 2)),
        Some(dropped("C settings", 1)),
        Some(dropped("r run", 3)),
        Some(dropped("/ search", 2)),
        Some(dropped("q quit", 1)),
    ];
    // TS filters out falsy parts: an empty string hint is dropped too.
    parts
        .into_iter()
        .flatten()
        .filter(|part| !part.text.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::state::AppProps;
    use crate::app::test_support::{app, session};
    use crate::primitives::row_text;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    #[test]
    fn sidebar_width_rules() {
        assert_eq!(sidebar_width(0), 24);
        assert_eq!(sidebar_width(130), 31);
        assert_eq!(sidebar_width(200), 34);
        assert_eq!(sidebar_width(80), 24);
        assert_eq!(sidebar_width(40), 17);
        assert_eq!(clamp_sidebar_width(5, 130), 18);
        assert_eq!(clamp_sidebar_width(100, 130), 65);
        assert_eq!(clamp_sidebar_width(30, 20), 10);
    }

    // Not in the TS: the sidebar takes its share of the terminal, less when every row fits in less, at least 24.
    #[test]
    fn the_sidebar_takes_its_share_unless_every_row_fits_in_less() {
        assert_eq!(proportional_sidebar_width(30.0, None, 200), 60);
        assert_eq!(proportional_sidebar_width(30.0, Some(75), 200), 60);
        assert_eq!(proportional_sidebar_width(30.0, Some(41), 200), 41);
        assert_eq!(proportional_sidebar_width(30.0, Some(12), 200), 24);
        assert_eq!(proportional_sidebar_width(40.0, None, 120), 48);
        assert_eq!(proportional_sidebar_width(15.0, None, 80), 24);
        assert_eq!(normalize_sidebar_percent(None), 30.0);
        assert_eq!(normalize_sidebar_percent(Some(f64::NAN)), 30.0);
        assert_eq!(normalize_sidebar_percent(Some(33.4)), 33.0);
        assert_eq!(normalize_sidebar_percent(Some(90.0)), 50.0);
        assert_eq!(normalize_sidebar_percent(Some(1.0)), 15.0);

        // A row's width is its content whole, plus the gap before its markers and the border.
        let short = session("a", "Short", SessionStatus::Running);
        let long = session("b", "A title ten longer", SessionStatus::Running);
        let rows = [&short, &long];
        let widths = sidebar_row_widths(&SidebarRowsInput {
            rows: &rows,
            all_sessions: &rows,
            first_number: 1,
            number_width: 1,
            selected_id: None,
            width: 0,
            spinner_frame: "⠋",
            filter: deckhand_protocol::records::SessionFilter::All,
            collapsed_session_ids: None,
            hidden_session_ids: None,
            installed_versions: None,
            shared_dev: true,
        });
        assert_eq!(widths[1] - widths[0], 13);
        let rendered = sidebar_rows(&SidebarRowsInput {
            rows: &rows,
            all_sessions: &rows,
            first_number: 1,
            number_width: 1,
            selected_id: None,
            width: usize::try_from(widths[1] - 3).unwrap(),
            spinner_frame: "⠋",
            filter: deckhand_protocol::records::SessionFilter::All,
            collapsed_session_ids: None,
            hidden_session_ids: None,
            installed_versions: None,
            shared_dev: true,
        });
        // At that width the longer title shows whole.
        assert!(
            rendered[1]
                .parts
                .iter()
                .any(|part| part.text.contains("A title ten longer"))
        );
    }

    // Not in the TS: fitted once the rows load and after a resize, never as sessions come and go.
    #[test]
    fn the_sidebar_is_measured_on_load_and_resize_only() {
        crate::app::modes::workspace_info::fake_daemon::local(async {
            let (mut app, _rx) = app(AppProps::default());
            assert_eq!(app.browse.sidebar_percent, None);
            assert!(app.browse.sidebar_fit_pending);
            app.browse.terminal_size = TerminalSize { cols: 200, rows: 40 };
            crate::app::effects::run_effects(&mut app);
            assert_eq!(app.browse.sidebar_content, None);
            assert_eq!(app.layout().sidebar_width, 60);
            let title = "x".repeat(20);
            app.data.repo_sessions = vec![session("a", &title, SessionStatus::Running)];
            app.data.sessions_loaded = true;
            crate::app::effects::run_effects(&mut app);
            // Every row fits in less than 30% of 200: the sidebar shrinks to them.
            let content = app.browse.sidebar_content.expect("measured once loaded");
            assert!(content < 60, "{content}");
            assert_eq!(i64::from(app.layout().sidebar_width), content.max(SIDEBAR_MIN));
            // A longer session later does not move it.
            let longer = session("b", &"y".repeat(50), SessionStatus::Running);
            app.data.repo_sessions.push(longer);
            crate::app::effects::run_effects(&mut app);
            assert_eq!(app.browse.sidebar_content, Some(content));
            // A resize measures again: now the share is what limits it.
            app.browse.sidebar_fit_pending = true;
            crate::app::effects::run_effects(&mut app);
            assert_eq!(app.layout().sidebar_width, 60);
        });
    }

    // Not in the TS: the measure counts what the row draws, such as +N for hidden exited sub-sessions, so the
    // sidebar never shrinks below a row it shows.
    #[test]
    fn the_sidebar_measures_rows_with_their_counts() {
        let (mut app, _rx) = app(AppProps::default());
        let parent = session("p", "conversation-testing-agent", SessionStatus::Running);
        let mut child = session("c", "x", SessionStatus::Exited);
        child.parent_session_id = Some("p".to_owned());
        app.data.repo_sessions = vec![parent, child];
        app.data.sessions_loaded = true;
        let derived = app.derived();
        measure_sidebar(&mut app, &derived);
        let shown = app.browse.sidebar_content.expect("measured");
        // Hiding the exited sub-session folds it into the parent's row as +1: that row is wider.
        app.browse.hidden_exited_session_ids.insert("c".to_owned());
        let derived = app.derived();
        assert_eq!(derived.visible.len(), 1);
        measure_sidebar(&mut app, &derived);
        let folded = app.browse.sidebar_content.expect("measured");
        assert!(folded > shown, "{folded} vs {shown}");
        // At that width the whole row, +1 included, is drawn uncut.
        let rows: Vec<&SessionRecord> = derived.visible.iter().collect();
        let all: Vec<&SessionRecord> = derived.sessions.iter().collect();
        let hidden: HashSet<String> = app.browse.hidden_exited_session_ids.iter().cloned().collect();
        let drawn = sidebar_rows(&SidebarRowsInput {
            rows: &rows,
            all_sessions: &all,
            first_number: 1,
            number_width: 1,
            selected_id: None,
            width: usize::try_from(folded - 3).unwrap(),
            spinner_frame: app.spinner_frame(),
            filter: app.browse.session_filter,
            collapsed_session_ids: None,
            hidden_session_ids: Some(&hidden),
            installed_versions: None,
            shared_dev: true,
        });
        let text: String = drawn[0].parts.iter().map(|part| part.text.as_str()).collect();
        assert!(
            text.contains("conversation-testing-agent") && text.contains("+1"),
            "{text}"
        );
    }

    #[test]
    fn layout_of_a_130_by_36_terminal() {
        let layout = app_layout(TerminalSize { cols: 130, rows: 36 }, None);
        assert_eq!(
            layout,
            AppLayout {
                sidebar_width: 31,
                preview_width: 98,
                content_height: 32,
                pane_inner_width: 94,
                pane_inner_height: 28,
                preview_cols: 94,
                preview_rows: 27,
            }
        );
        let tiny = app_layout(TerminalSize { cols: 10, rows: 5 }, Some(-4));
        assert_eq!(tiny.content_height, 8);
        assert_eq!(tiny.preview_width, 20);
        assert_eq!(tiny.pane_inner_width, 16);
    }

    fn browse(session: Option<&SessionRecord>, tab: RightPaneTab, width: usize) -> String {
        footer_hint(&FooterHintInput {
            mode: Mode::Browse,
            active_tab: tab,
            width,
            session,
            scroll_sensitivity: 0.12,
            attach_ready: true,
            notes_hint: None,
            terminal_action: None,
            dev_switch: None,
        })
    }

    #[test]
    fn browse_hints_per_session_and_tab() {
        assert_eq!(
            browse(None, RightPaneTab::Preview, 130),
            "? help • n new • W worktrees • C settings • r run • / search • q quit"
        );
        let running = session("a", "A", SessionStatus::Running);
        assert_eq!(
            browse(Some(&running), RightPaneTab::Preview, 200),
            "→ scroll • enter open • x stop • E open in editor • ? help • n new • W worktrees • C settings • r run • \
             / search • q quit"
        );
        let mut exited = session("b", "B", SessionStatus::Exited);
        exited.archived_at = Some("2026-01-02T00:00:00.000Z".to_owned());
        assert_eq!(
            browse(Some(&exited), RightPaneTab::Notes, 300),
            "enter edit notes • o vim • E open in editor • s resume • S fresh • A unarchive • \
             backspace remove for good • ? help • n new • W worktrees • C settings • r run • / search • q quit"
        );
        assert_eq!(
            browse(Some(&running), RightPaneTab::Dev, 200),
            "enter open • d d start Dev • x stop • E open in editor • ? help • n new • W worktrees • C settings • \
             r run • / search • q quit"
        );
        // Narrow: higher drops go first, then short forms.
        assert_eq!(
            browse(Some(&running), RightPaneTab::Preview, 40),
            "→ scroll • enter open • x stop • ? help"
        );
    }

    #[test]
    fn other_modes_have_their_own_hints() {
        let input = |mode| FooterHintInput {
            mode,
            active_tab: RightPaneTab::Preview,
            width: 200,
            session: None,
            scroll_sensitivity: 0.2,
            attach_ready: false,
            notes_hint: None,
            terminal_action: None,
            dev_switch: None,
        };
        assert_eq!(footer_hint(&input(Mode::Tasks)), "");
        assert_eq!(footer_hint(&input(Mode::NotesFocus)), "esc done");
        assert_eq!(
            footer_hint(&input(Mode::PreviewFocus)),
            "j/k ↑↓ J/K scroll • g/G top/bottom • wheel ×0.2 (+/− speed) • ← back"
        );
        assert_eq!(
            footer_hint(&input(Mode::PickFilter)),
            "a r ! e A * pick • f back to active • enter keep • esc cancel"
        );
        assert!(footer_hint(&input(Mode::ChangesFocus)).starts_with("space stage/unstage • "));
    }

    #[test]
    fn footer_messages_join_on_one_row() {
        let (mut app, _rx) = app(AppProps::default());
        let [hint, messages] = footer_rows(&app);
        assert_eq!(row_text(&messages), " ");
        assert!(row_text(&hint).starts_with("? help"));
        app.set_error(Some("bad\n  thing".to_owned()));
        app.set_busy(true);
        app.browse.numeric_selection = "12".to_owned();
        app.set_status(Some("done".to_owned()));
        let [_, messages] = footer_rows(&app);
        assert_eq!(
            row_text(&messages),
            "Error: bad thing · Working… · Select session: 12 · done"
        );
        assert_eq!(footer_messages(&app).len(), 4);
        app.set_mode(Mode::Search);
        app.browse.session_query = "zz".to_owned();
        let [hint, _] = footer_rows(&app);
        assert_eq!(row_text(&hint), "Search: /zz • enter keep • esc clear");
    }

    #[test]
    fn header_shows_the_connection_and_repo() {
        let (app, _rx) = app(AppProps::default());
        let [title, repo] = header_rows(&app);
        let text = row_text(&title);
        assert!(text.starts_with("deckhand"));
        assert!(text.ends_with("● connecting…"));
        assert_eq!(crate::primitives::string_width(&text), 130);
        assert_eq!(row_text(&repo), "/repo");
        let squeezed = header_space_between(&[Seg::plain("deckhand")], &[Seg::plain("● connecting…")], 12);
        assert_eq!(crate::primitives::string_width(&row_text(&squeezed)), 12);
    }

    /// Renders `app` (connected over a socket pair, sessions loaded, `<TMP>/demo` the repo) after `setup` and compares
    /// the screen with `fixtures/ui/<name>.txt` (rows right-trimmed), using the default fitted sidebar.
    fn assert_screen(name: &str, setup: impl FnOnce(&mut App)) -> Buffer {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap_or_else(|error| panic!("{error}"));
        tokio::task::LocalSet::new().block_on(&runtime, async {
            let (mut app, _rx) = app(AppProps::default());
            app.props.repo_root = "<TMP>/demo".to_owned();
            app.data.sessions_loaded = true;
            // A client over a socket pair stands for the connected daemon (● ready).
            let (socket, _peer) = tokio::net::UnixStream::pair().unwrap_or_else(|error| panic!("{error}"));
            app.connection.client = Some(std::rc::Rc::new(deckhand_client::live::LiveClient::new(
                socket,
                deckhand_client::live::LiveClientHandlers::default(),
            )));
            setup(&mut app);
            let derived = app.derived();
            measure_sidebar(&mut app, &derived);
            let size = app.browse.terminal_size;
            let mut terminal =
                Terminal::new(TestBackend::new(size.cols, size.rows)).unwrap_or_else(|error| panic!("{error}"));
            terminal
                .draw(|frame| render_app(&mut app, frame))
                .unwrap_or_else(|error| panic!("{error}"));
            let buffer = terminal.backend().buffer();
            let rows: Vec<String> = (0..buffer.area.height)
                .map(|y| {
                    let mut line = String::new();
                    let mut x = 0;
                    while x < buffer.area.width {
                        let symbol = buffer[(x, y)].symbol();
                        line.push_str(symbol);
                        x += u16::try_from(crate::primitives::string_width(symbol).max(1)).unwrap_or(1);
                    }
                    line.trim_end().to_owned()
                })
                .collect();
            deckhand_testkit::assert_snapshot(format!("fixtures/ui/{name}.txt"), &rows.join("\n"));
            buffer.clone()
        })
    }

    #[test]
    fn renders_the_empty_dashboard_snapshot() {
        assert_screen("01-empty-dashboard", |_| {});
    }

    #[test]
    fn an_error_too_long_for_the_footer_shows_whole_in_a_toast() {
        assert_eq!(error_toast_rows("short", 130), None);
        let long = format!("git worktree remove failed: {} (the end)", "detail ".repeat(30));
        let (width, rows) = error_toast_rows(&long, 130).unwrap_or_else(|| panic!("a toast"));
        assert_eq!(width, 85);
        let text: Vec<String> = rows.iter().map(|row| row_text(row)).collect();
        assert!(text[0].starts_with("Error: git worktree remove failed:"), "{text:?}");
        assert!(text.last().is_some_and(|row| row.ends_with("(the end)")), "{text:?}");
        // Line breaks are kept; a short multi-line error still gets the box.
        let (_, rows) = error_toast_rows("first\nsecond", 130).unwrap_or_else(|| panic!("a toast"));
        assert_eq!(
            rows.iter().map(|row| row_text(row)).collect::<Vec<_>>(),
            ["Error: first", "second"]
        );
        // At most ERROR_TOAST_MAX_ROWS, the cut marked.
        let (_, rows) = error_toast_rows(&"word ".repeat(400), 130).unwrap_or_else(|| panic!("a toast"));
        assert_eq!(rows.len(), ERROR_TOAST_MAX_ROWS - 2);
        assert!(rows.last().is_some_and(|row| row_text(row).ends_with('…')));

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap_or_else(|error| panic!("{error}"));
        tokio::task::LocalSet::new().block_on(&runtime, async {
            let (mut app, _rx) = app(AppProps::default());
            app.messages.error = Some(long.clone());
            let mut terminal = Terminal::new(TestBackend::new(130, 36)).unwrap_or_else(|error| panic!("{error}"));
            terminal
                .draw(|frame| render_app(&mut app, frame))
                .unwrap_or_else(|error| panic!("{error}"));
            let buffer = terminal.backend().buffer();
            let screen: Vec<String> = (0..buffer.area.height)
                .map(|y| (0..buffer.area.width).map(|x| buffer[(x, y)].symbol()).collect())
                .collect();
            // The footer row (cut) and the toast at the top (whole).
            assert!(screen[35].contains("Error: git worktree remove failed:"));
            assert!(screen[2..10].iter().any(|row| row.contains("(the end)")), "{screen:#?}");
        });
    }

    #[test]
    fn a_dialog_floats_over_the_faded_session_pane() {
        let buffer = assert_screen("07-dialog-over-dashboard", |app| {
            let mut auth = session("a", "auth refactor", SessionStatus::Running);
            let mut tests = session("b", "write tests", SessionStatus::Exited);
            // Updated just now, so the details' age reads the same every day.
            let now = deckhand_core::jsstr::date::to_iso_string(deckhand_core::jsstr::date::now_ms());
            for record in [&mut auth, &mut tests] {
                record.cwd = "<TMP>/demo".to_owned();
                record.repo_root = "<TMP>/demo".to_owned();
                record.agent_status_updated_at.clone_from(&now);
            }
            app.data.repo_sessions = vec![auth, tests];
            app.set_selected_id(Some("a".to_owned()));
            app.panes.preview = deckhand_protocol::data::PreviewRecord {
                session_id: Some("a".to_owned()),
                content: (1..=24)
                    .map(|n| format!("agent output line {n}"))
                    .collect::<Vec<_>>()
                    .join("\n"),
                live: true,
                status: Some(SessionStatus::Running),
                agent_status: None,
                scroll_offset: None,
                max_scroll_offset: None,
            };
            app.set_mode(Mode::PickProgram);
        });
        let layout = app_layout(TerminalSize { cols: 130, rows: 36 }, None);
        let right_x = layout.sidebar_width + 1;
        let text_of = |y: u16| -> String { (0..buffer.area.width).map(|x| buffer[(x, y)].symbol()).collect() };
        // The preview's last lines show below the dialog, faded; the sidebar is not.
        let below = (0..buffer.area.height)
            .find(|&y| text_of(y).contains("agent output line 24"))
            .unwrap_or_else(|| panic!("the preview shows behind the dialog"));
        let x = (right_x..buffer.area.width)
            .find(|&x| buffer[(x, below)].symbol() == "a")
            .unwrap_or(right_x);
        assert!(buffer[(x, below)].modifier.contains(ratatui::style::Modifier::DIM));
        let sidebar_title = (0..buffer.area.height)
            .find(|&y| text_of(y).contains("auth refactor"))
            .unwrap_or_else(|| panic!("the sidebar lists the session"));
        let title_x = (0..layout.sidebar_width)
            .find(|&x| buffer[(x, sidebar_title)].symbol() == "a")
            .unwrap_or(0);
        assert!(
            !buffer[(title_x, sidebar_title)]
                .modifier
                .contains(ratatui::style::Modifier::DIM)
        );
    }

    #[test]
    fn renders_the_help_guide_full_width_snapshots() {
        assert_screen("02-help", |app| {
            app.flows.help.open(None);
            app.set_mode(Mode::Help);
        });
        assert_screen("03-help-topic", |app| {
            app.flows.help.open(None);
            app.flows.help.pick(1);
            app.set_mode(Mode::Help);
        });
    }

    #[test]
    fn renders_search_and_the_filter_menu_snapshots() {
        assert_screen("05-search", |app| {
            app.set_mode(Mode::Search);
            app.browse.session_query = "zz".to_owned();
        });
        assert_screen("06-filter-menu", |app| {
            app.set_mode(Mode::PickFilter);
        });
    }
}
