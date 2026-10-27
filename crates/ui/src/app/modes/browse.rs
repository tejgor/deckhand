//! Browse keys (`if (mode === 'browse')`): the session list and the tabbed right pane. Selecting, tabs, the lifecycle
//! keys (stop, resume, archive, done, merge), opening what a tab shows, and the doors to every other screen.

use std::cell::RefCell;
use std::rc::Rc;

use deckhand_client::live::KillOptions;
use deckhand_core::display::display_session_title;
use deckhand_core::jsstr::date::now_ms;
use deckhand_core::jsstr::{js_len, js_trim};
use deckhand_core::key::Input;
use deckhand_core::session_features::{
    filter_session_list, session_blocked, session_has_unread_reply, session_needs_attention,
};
use deckhand_core::session_order::{session_descendants, session_has_children};
use deckhand_core::tasks_board::work_key_of;
use deckhand_core::workspace::{no_workspace_reason, workspace_key};
use deckhand_protocol::data::JsNumber;
use deckhand_protocol::data::{
    AttachTarget, AttentionState, DevRecord, MoveDirection, ProjectInfo, RestartMode, RightPaneTab, SessionRecord,
    SessionStatus, SessionWorktreeMode, SetupState, WorktreeMode,
};
use deckhand_protocol::records::{AppConfig, SessionFilter};
use deckhand_sys::desktop::open_in_editor;
use deckhand_sys::storage::update_app_config;

use crate::app::layout::{SIDEBAR_PERCENT_STEP, measure_sidebar, normalize_sidebar_percent};
use crate::app::modes::{action_picker, confirm, create, filter, workspace_info};
use crate::app::state::{App, AppCallback, Mode, ReviewLabels};
use crate::app::{DEV_DOUBLE_TAP_MS, RIGHT_TABS, ReviewOptions, hint_short, sort_sessions, truthy, upsert_session};
use crate::keymap::{Binding, HelpRef, KeySpec, Keymap, NamedKey};
use crate::panes::dev::dev_processes;
use crate::panes::terminal::{TerminalView, has_action};

/// The browse view's keys (keysDocumented: every one in the ? guide).
pub static KEYMAP: Keymap = &[
    Binding {
        keys: &[
            KeySpec::Named(NamedKey::Tab),
            KeySpec::Named(NamedKey::ShiftTab),
            KeySpec::Char("["),
            KeySpec::Char("]"),
        ],
        action: "next / previous tab",
        help: HelpRef::Topic("Start here"),
    },
    Binding {
        // "1-9" stands for the digits as the guide writes them (TS tested them with /^\d$/, not `input === '1'`).
        keys: &[KeySpec::Char("1-9"), KeySpec::Char("0")],
        action: "select a session by its number",
        help: HelpRef::Topic("Find & organize"),
    },
    Binding {
        keys: &[KeySpec::Char("q")],
        action: "quit",
        help: HelpRef::Topic("Start here"),
    },
    Binding {
        keys: &[KeySpec::Char("?")],
        action: "the guide",
        help: HelpRef::Topic("Start here"),
    },
    Binding {
        keys: &[KeySpec::Char("/")],
        action: "search",
        help: HelpRef::Topic("Find & organize"),
    },
    Binding {
        keys: &[KeySpec::Char("f")],
        action: "filter menu",
        help: HelpRef::Topic("Find & organize"),
    },
    Binding {
        keys: &[KeySpec::Char("A")],
        action: "archive / unarchive",
        help: HelpRef::Topic("Sessions"),
    },
    Binding {
        keys: &[KeySpec::Char("!")],
        action: "next session needing attention",
        help: HelpRef::Topic("Find & organize"),
    },
    Binding {
        keys: &[KeySpec::Char("i")],
        action: "workspace overview",
        help: HelpRef::Topic("Git & PRs"),
    },
    Binding {
        keys: &[KeySpec::Char("C"), KeySpec::Char("G")],
        action: "settings / agents",
        help: HelpRef::Topic("Start here"),
    },
    Binding {
        keys: &[KeySpec::Char("r")],
        action: "run list",
        help: HelpRef::Topic("Dev & actions"),
    },
    Binding {
        keys: &[KeySpec::Char("b"), KeySpec::Char("W")],
        action: "tasks board / worktrees",
        help: HelpRef::Topic("Start here"),
    },
    Binding {
        keys: &[KeySpec::Char("H")],
        action: "export a handoff",
        help: HelpRef::Topic("Sessions"),
    },
    Binding {
        keys: &[KeySpec::Char("x"), KeySpec::Char("X")],
        action: "stop (asking first) / force-stop",
        help: HelpRef::Topic("Sessions"),
    },
    Binding {
        keys: &[KeySpec::Char("n")],
        action: "new session",
        help: HelpRef::Topic("Start here"),
    },
    Binding {
        keys: &[KeySpec::Char("N")],
        action: "new sub-session",
        help: HelpRef::Topic("Sessions"),
    },
    Binding {
        keys: &[KeySpec::Char("R")],
        action: "refresh the list",
        help: HelpRef::Topic("Find & organize"),
    },
    Binding {
        keys: &[KeySpec::Named(NamedKey::Right), KeySpec::Char("l")],
        action: "step into the pane",
        help: HelpRef::Topic("Panes"),
    },
    Binding {
        keys: &[KeySpec::Named(NamedKey::Left), KeySpec::Char("h")],
        action: "already out of the pane",
        help: HelpRef::Topic("Panes"),
    },
    Binding {
        keys: &[
            KeySpec::Char("p"),
            KeySpec::Char("t"),
            KeySpec::Char("g"),
            KeySpec::Char("a"),
        ],
        action: "Preview, Terminal, Git, Notes tab",
        help: HelpRef::Topic("Panes"),
    },
    Binding {
        keys: &[KeySpec::Char("d"), KeySpec::Char("dd")],
        action: "Dev tab; twice quickly starts or stops Dev",
        help: HelpRef::Topic("Dev & actions"),
    },
    Binding {
        keys: &[KeySpec::Char("v")],
        action: "Terminal: the shell or the last action; Dev: the next process",
        help: HelpRef::Topic("Panes"),
    },
    Binding {
        keys: &[KeySpec::Char("c")],
        action: "collapse / expand sub-sessions",
        help: HelpRef::Topic("Find & organize"),
    },
    Binding {
        keys: &[KeySpec::Char("K"), KeySpec::Char("J")],
        action: "move among siblings",
        help: HelpRef::Topic("Find & organize"),
    },
    Binding {
        keys: &[
            KeySpec::Char("k"),
            KeySpec::Named(NamedKey::Up),
            KeySpec::Char("j"),
            KeySpec::Named(NamedKey::Down),
        ],
        action: "move the selection",
        help: HelpRef::Topic("Start here"),
    },
    Binding {
        keys: &[KeySpec::Char("<"), KeySpec::Char(">"), KeySpec::Char("=")],
        action: "resize the sidebar (saved); = fits it to the sessions again",
        help: HelpRef::Topic("Find & organize"),
    },
    Binding {
        keys: &[KeySpec::Char("m"), KeySpec::Char("M")],
        action: "merge / mark merged",
        help: HelpRef::Topic("Sessions"),
    },
    Binding {
        keys: &[KeySpec::Char(" ")],
        action: "mark done",
        help: HelpRef::Topic("Sessions"),
    },
    Binding {
        keys: &[KeySpec::Char("U")],
        action: "mark the reply unread / read",
        help: HelpRef::Topic("Sessions"),
    },
    Binding {
        keys: &[KeySpec::Named(NamedKey::Backspace), KeySpec::Named(NamedKey::Delete)],
        action: "archive, or remove an archived session",
        help: HelpRef::Topic("Sessions"),
    },
    Binding {
        keys: &[KeySpec::Char("s"), KeySpec::Char("S")],
        action: "resume / start fresh",
        help: HelpRef::Topic("Sessions"),
    },
    Binding {
        keys: &[KeySpec::Char("E")],
        action: "open in Cursor / VS Code",
        help: HelpRef::Topic("Panes"),
    },
    Binding {
        keys: &[KeySpec::Named(NamedKey::Return), KeySpec::Char("o")],
        action: "open the tab full screen",
        help: HelpRef::Topic("Start here"),
    },
];

/// TS `/^\d$/`: one ASCII digit.
fn is_digit(text: &str) -> bool {
    text.len() == 1 && text.bytes().all(|byte| byte.is_ascii_digit())
}

/// TS `selectedWorkspace` of a session (a truthy workspace key).
fn workspace_of(session: &SessionRecord) -> Option<String> {
    workspace_key(session).filter(|key| !key.is_empty())
}

/// TS `selectedSession.devRunning || (dev.sessionId === selectedSession.id && dev.live)`.
fn dev_running(app: &App, session: &SessionRecord) -> bool {
    session.dev_running == Some(true)
        || (app.panes.dev.session_id.as_deref() == Some(session.id.as_str()) && app.panes.dev.live)
}

/// TS: setPreviewScrollOffset(0); setActiveTab(tab).
fn show_tab(app: &mut App, tab: RightPaneTab) {
    app.preview_focus.scroll_offset = 0;
    app.set_active_tab(tab);
}

/// TS: the browse branch of the useTerminalInput handler.
pub fn handle_browse(app: &mut App, input: &Input) {
    let key = &input.key;
    let text = input.input.as_str();
    // Tab / ] next tab, Shift+Tab / [ previous.
    if key.tab || text == "[" || text == "]" {
        let step = if (key.tab && key.shift) || text == "[" {
            RIGHT_TABS.len() - 1
        } else {
            1
        };
        cycle_tab(app, step);
        return;
    }
    if !app.browse.numeric_selection.is_empty() {
        if is_digit(text) {
            app.browse.numeric_selection.push_str(text);
            return;
        }
        if key.return_ {
            let value = app.browse.numeric_selection.clone();
            confirm_numeric_selection(app, &value);
            return;
        }
        if key.escape {
            app.browse.numeric_selection.clear();
            return;
        }
        if key.backspace || key.delete {
            app.browse.numeric_selection.pop();
            return;
        }
        app.browse.numeric_selection.clear();
    }
    if is_digit(text) {
        if app.visible_sessions().len() <= 10 {
            confirm_numeric_selection(app, if text == "0" { "10" } else { text });
        } else {
            app.browse.numeric_selection = text.to_owned();
        }
        return;
    }
    if text == "q" {
        app.quit();
        return;
    }
    if text == "?" {
        app.flows.help.open(None);
        app.set_mode(Mode::Help);
        return;
    }
    if text == "/" {
        app.set_mode(Mode::Search);
        return;
    }
    // `fr` (or `ff`) can arrive as one chunk when typed fast: the second key picks in the menu.
    if text == "f" || (js_len(text) == 2 && text.starts_with('f')) {
        filter::open_filter_menu(app, input);
        return;
    }
    let selected = app.selected_session();
    let connected = app.client().is_some();
    if text == "A" && connected && selected.is_some() {
        toggle_archive(app);
        return;
    }
    if text == "!" {
        next_attention(app);
        return;
    }
    if text == "i" && connected && selected.is_some() {
        workspace_info::open_workspace_info(app);
        return;
    }
    if text == "C" && connected {
        let cwd = selected
            .as_ref()
            .map_or_else(|| app.props.cwd.clone(), |session| session.cwd.clone());
        app.settings_flow_call(|flow, props, cx| flow.open(&cwd, props, cx));
        return;
    }
    if text == "G" && connected {
        app.agents_flow_call(|flow, props, cx| flow.open(props, cx));
        return;
    }
    if text == "r" {
        // The run list: the worktree's Dev command first, then every action of the session's repository (untrusted
        // repository ones are reviewed when chosen).
        action_picker::open_run_list(app);
        return;
    }
    if text == "b" {
        let scope = selected.as_ref().map(work_key_of);
        app.tasks_flow_call(|flow, props, _cx| flow.open(scope.as_deref(), props));
        app.set_mode(Mode::Tasks);
        return;
    }
    // W: every worktree of the repository, what is merged, and deleting them (stopping their sessions first).
    if text == "W" {
        if !connected {
            app.set_error(Some("still connecting to daemon".to_owned()));
            return;
        }
        let select = selected.as_ref().and_then(workspace_key);
        app.worktrees_flow_call(|flow, props, cx| flow.open(select.as_deref(), props, cx));
        app.set_mode(Mode::Worktrees);
        return;
    }
    if text == "H"
        && let (Some(client), Some(session)) = (app.client(), selected.as_ref())
    {
        let reply = client.export_handoff(&session.id, None);
        drop(app.dispatch().spawn(reply, |app: &mut App, result| match result {
            Ok(file) => {
                app.set_status(Some(format!("Handoff: {file}")));
                open_editor(app, &file);
            }
            Err(error) => app.set_error(Some(error.to_string())),
        }));
        return;
    }
    if (text == "x" || text == "X")
        && let Some(session) = selected
            .as_ref()
            .filter(|session| session.status == SessionStatus::Starting)
        && let Some(client) = app.client()
    {
        let reply = client.cancel_start(&session.id);
        drop(app.dispatch().spawn(reply, |app: &mut App, result| {
            if let Err(error) = result {
                app.set_error(Some(error.to_string()));
            }
        }));
        return;
    }
    if text == "n" || text == "N" {
        // No review here: the repository's defaults only preselect the picker (trusted or not). Creating a new
        // worktree reviews whatever it would run (see confirmCreate). Children keep their parent's agent and directory.
        create::open_new_session(app, text == "N");
        return;
    }
    if text == "R" {
        refresh_sessions(app);
        return;
    }
    // → / l steps into the pane (scroll the preview, browse the changes, edit the notes); ← / h is back out (Esc too).
    if key.right_arrow || text == "l" {
        step_into_pane(app);
        return;
    }
    if key.left_arrow || text == "h" {
        return;
    }
    let tab = match text {
        "p" => Some(RightPaneTab::Preview),
        "t" => Some(RightPaneTab::Terminal),
        "g" => Some(RightPaneTab::Git),
        "a" => Some(RightPaneTab::Notes),
        _ => None,
    };
    if let Some(tab) = tab {
        show_tab(app, tab);
        return;
    }
    // Tab letters only switch tabs; Dev starts and stops from the run list (r).
    // d shows the Dev tab; d twice quickly (a deliberate double-tap, not a later second d) starts or stops Dev. A fast
    // double-tap can arrive as one chunk ("dd").
    if text == "d" || text == "dd" {
        dev_tap(app, text);
        return;
    }
    // The Dev tab selects the next of its named processes (enter attaches the selected one).
    if text == "v" && app.browse.active_tab == RightPaneTab::Dev {
        if let Some(next) = app.next_dev_process() {
            app.panes.dev_process = Some(next);
        }
        return;
    }
    // The Terminal tab switches between the shell and the worktree's last action.
    if text == "v"
        && app.browse.active_tab == RightPaneTab::Terminal
        && let Some(session) = selected.as_ref()
    {
        if has_action(Some(session), &app.panes.action) {
            let showing = app.showing_action();
            app.panes.terminal_view = if app.panes.terminal_view == TerminalView::Action && showing {
                TerminalView::Shell
            } else {
                TerminalView::Action
            };
        } else {
            app.set_error(Some("No action has run in this worktree yet; r runs one".to_owned()));
        }
        return;
    }
    if text == "c" {
        toggle_selected_collapse(app);
        return;
    }
    if text == "K" {
        reorder_selected(app, MoveDirection::Up);
        return;
    }
    if text == "J" {
        reorder_selected(app, MoveDirection::Down);
        return;
    }
    if text == "k" || key.up_arrow {
        move_selection(app, -1);
        return;
    }
    if text == "j" || key.down_arrow {
        move_selection(app, 1);
        return;
    }
    if text == "<" || text == ">" {
        resize_sidebar(app, if text == "<" { -1 } else { 1 });
        return;
    }
    if text == "=" {
        fit_sidebar(app);
        return;
    }
    let Some(session) = selected else {
        // Every key below acts on the selected session; E says there is none.
        if text == "E" {
            open_selected_in_editor(app);
        }
        return;
    };
    let worktree = session.worktree.as_ref();
    let deleted = worktree.is_some_and(|worktree| truthy(worktree.deleted_at.as_ref()));
    let not_ready = session
        .requested_worktree_mode
        .is_some_and(|mode| mode != WorktreeMode::None)
        && workspace_key(&session).is_none_or(|key| key.is_empty());
    // m merges the session's worktree; without one it says why, as M does.
    if text == "m"
        && (worktree.is_none_or(|worktree| !truthy(worktree.path.as_ref()))
            || worktree.is_some_and(|worktree| worktree.mode == SessionWorktreeMode::None)
            || deleted)
    {
        app.set_status(Some(
            if deleted {
                "Its worktree was deleted: nothing to merge"
            } else if not_ready {
                "Its worktree is not ready yet"
            } else {
                "Not in a worktree, so there is nothing to merge. Use Space to mark it done"
            }
            .to_owned(),
        ));
        return;
    }
    if text == "m" {
        confirm::open_merge_confirm(app);
        return;
    }
    if text == "M" {
        // Merged belongs to worktrees; a main-checkout session can still be marked done.
        if worktree.is_none_or(|worktree| !truthy(worktree.id.as_ref())) && !confirm::has_merged_marker(Some(&session))
        {
            app.set_status(Some(
                if not_ready {
                    "Its worktree is not ready yet"
                } else {
                    "Not in a worktree, so there is nothing to mark merged. Use Space to mark it done"
                }
                .to_owned(),
            ));
        } else {
            mark_selected_merged(app);
        }
        return;
    }
    // Space marks the session done (or not), as it ticks a task.
    if text == " " {
        toggle_selected_done(app);
        return;
    }
    if text == "U" {
        toggle_selected_reply_read(app);
        return;
    }
    // x asks first (with the worktree's deletion choices); X force-stops at once and keeps any worktree.
    if text == "X" && session.status == SessionStatus::Running {
        confirm::kill_selected(
            app,
            KillOptions {
                force: true,
                ..KillOptions::default()
            },
        );
        return;
    }
    if (text == "x" || text == "X") && session.status == SessionStatus::Exited {
        app.set_status(Some(format!(
            "Already stopped: Backspace {}{}",
            if truthy(session.archived_at.as_ref()) {
                "removes it for good (asking first)"
            } else {
                "archives it"
            },
            if deleted { "" } else { ", s resumes it" }
        )));
        return;
    }
    if text == "x" && session.status == SessionStatus::Running {
        confirm::open_kill_confirm(app);
        return;
    }
    // Backspace archives a finished session (hidden, nothing lost); on one already archived (f → archived) it removes
    // it for good, which deletes its notes (open items ask first).
    if (key.backspace || key.delete) && session.status == SessionStatus::Exited {
        archive_or_remove(app);
        return;
    }
    if (text == "s" || text == "S") && session.status == SessionStatus::Exited {
        let restart_mode = if text == "S" {
            RestartMode::Fresh
        } else {
            RestartMode::Resume
        };
        // Setup still owed (failed, cancelled or refused as untrusted) depends on the repository config.
        if session
            .setup
            .as_ref()
            .is_some_and(|setup| setup.state != SetupState::Complete)
        {
            review_setup_then_restart(app, &session.cwd, restart_mode);
        } else {
            restart_selected(app, restart_mode, None);
        }
        return;
    }
    // E: your editor. On Notes the active note (the session's, or the worktree's after editing that), else the
    // workspace.
    if text == "E" {
        if app.browse.active_tab == RightPaneTab::Notes {
            app.notes_flow_call(|flow, props, cx| flow.open_active_in_editor(props, cx));
        } else {
            open_selected_in_editor(app);
        }
        return;
    }
    // o on Notes: the note in your terminal editor (nvim/vim), back here when it quits.
    if text == "o" && app.browse.active_tab == RightPaneTab::Notes {
        app.edit_note_in_terminal(&session);
        return;
    }
    open_selected(app, input);
}

/// s / S on an exited session whose setup is still owed: the repository's setupCommand is reviewed first when it is
/// untrusted.
fn review_setup_then_restart(app: &mut App, cwd: &str, restart_mode: RestartMode) {
    let resume = Rc::new(move |app: &mut App, project: Option<ProjectInfo>| {
        restart_selected(app, restart_mode, project.map(|project| project.project.fingerprint));
    });
    let options = ReviewOptions {
        gate: Some(Rc::new(|project: &ProjectInfo| {
            project.needs_review && truthy(project.project.config.setup_command.as_ref())
        })),
        labels: Some(Rc::new(|_: &ProjectInfo| ReviewLabels {
            enter: Some(hint_short("enter trust & retry setup", "enter trust")),
            skip: Some(hint_short("s retry without the repo's setup", "s without it")),
        })),
        purpose: Some(Rc::new(|project: &ProjectInfo| {
            format!(
                "About to run the repository setupCommand: {}",
                project.project.config.setup_command.as_deref().unwrap_or("undefined")
            )
        })),
        ..ReviewOptions::default()
    };
    app.review_then(cwd, Some(resume), options);
}

/// TS: Tab / ] next tab, Shift+Tab / [ previous (`step` 1 or RIGHT_TABS.length - 1).
pub fn cycle_tab(app: &mut App, step: usize) {
    let current = RIGHT_TABS
        .iter()
        .position(|tab| *tab == app.browse.active_tab)
        .map_or(-1, |index| index as i64);
    // TS: indexOf is -1 for an unknown tab, so the step lands one short (never the case for a known tab).
    let next = (current + step as i64).rem_euclid(RIGHT_TABS.len() as i64) as usize;
    let tab = RIGHT_TABS.get(next).copied().unwrap_or(RightPaneTab::Preview);
    show_tab(app, tab);
}

/// JS `Number.parseInt(value, 10)` of a typed number (leading digits; `None` for NaN, an infinite or unsafe result).
fn parse_int(value: &str) -> Option<u128> {
    let trimmed = js_trim(value);
    let digits: String = trimmed
        .strip_prefix('+')
        .unwrap_or(trimmed)
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    if digits.is_empty() || trimmed.starts_with('-') {
        // A negative number is never a visible session's (and parseInt('-') is NaN).
        return None;
    }
    digits.parse::<u128>().ok()
}

/// TS `confirmNumericSelection`: select visible session number `value`, or say there is none.
pub fn confirm_numeric_selection(app: &mut App, value: &str) {
    if value.is_empty() {
        return;
    }
    app.browse.numeric_selection.clear();
    let visible = app.visible_sessions();
    let target = parse_int(value)
        .and_then(|number| number.checked_sub(1))
        .and_then(|index| usize::try_from(index).ok())
        .and_then(|index| visible.get(index));
    let Some(target) = target else {
        app.set_error(Some(format!("no visible session {value}")));
        return;
    };
    let id = target.id.clone();
    app.set_error(None);
    app.set_selected_id(Some(id));
}

/// TS `moveSelection` (j/k, wrapping).
pub fn move_selection(app: &mut App, delta: isize) {
    let visible = app.visible_sessions();
    if visible.is_empty() {
        return;
    }
    let len = visible.len() as isize;
    let index = app.selected_index() as isize;
    let next = (index + delta + len).rem_euclid(len) as usize;
    if let Some(session) = visible.get(next) {
        let id = session.id.clone();
        app.set_selected_id(Some(id));
    }
}

/// TS `toggleSelectedCollapse` (c): hide exited children → collapse → expand.
pub fn toggle_selected_collapse(app: &mut App) {
    let sessions = app.sessions();
    // The sub-sessions this view lists (as the sidebar's ▾ ▸): archived ones are not there.
    let listed = filter_session_list(&sessions, app.browse.session_filter, "", None);
    let selected = app.selected_session();
    let Some(selected) = selected.filter(|session| session_has_children(&session.id, &listed)) else {
        app.set_error(Some("selected session has no sub-sessions".to_owned()));
        return;
    };
    let descendants = session_descendants(&selected.id, &listed);
    let descendant_ids: Vec<String> = descendants.iter().map(|session| session.id.clone()).collect();
    let exited_ids: Vec<String> = descendants
        .iter()
        .filter(|session| session.status == SessionStatus::Exited)
        .map(|session| session.id.clone())
        .collect();
    let selected_collapsed = app.browse.collapsed_session_ids.contains(&selected.id);
    let exited_already_hidden = !exited_ids.is_empty()
        && exited_ids
            .iter()
            .all(|id| app.browse.hidden_exited_session_ids.contains(id));

    app.set_error(None);
    if selected_collapsed {
        let collapsed = &mut app.browse.collapsed_session_ids;
        collapsed.shift_remove(&selected.id);
        for id in &descendant_ids {
            collapsed.shift_remove(id);
        }
        let hidden = &mut app.browse.hidden_exited_session_ids;
        for id in &exited_ids {
            hidden.shift_remove(id);
        }
        app.set_status(Some("expanded all sub-sessions".to_owned()));
        return;
    }

    if !exited_ids.is_empty() && !exited_already_hidden {
        app.browse.hidden_exited_session_ids.extend(exited_ids.iter().cloned());
        app.set_status(Some(format!(
            "collapsed {} exited sub-session{}",
            exited_ids.len(),
            if exited_ids.len() == 1 { "" } else { "s" }
        )));
        return;
    }

    app.browse.collapsed_session_ids.insert(selected.id.clone());
    app.set_status(Some("collapsed all sub-sessions".to_owned()));
}

/// TS `reorderSelected` (J / K): move the session among its siblings.
pub fn reorder_selected(app: &mut App, direction: MoveDirection) {
    let (Some(client), Some(session)) = (app.client(), app.selected_session()) else {
        return;
    };
    app.set_busy(true);
    app.set_error(None);
    let reply = client.reorder_session(&session.id, direction);
    drop(app.dispatch().spawn(reply, |app: &mut App, result| {
        match result {
            Ok(reordered) => {
                let mut current = std::mem::take(&mut app.data.repo_sessions);
                for session in reordered {
                    current = upsert_session(current, session);
                }
                app.data.repo_sessions = sort_sessions(current);
            }
            Err(error) => app.set_error(Some(error.to_string())),
        }
        app.set_busy(false);
    }));
}

/// TS `resizeSidebar` (< / >). Not in the TS: it moves the sidebar 2% of the terminal from its width on screen (a
/// fitted one included) and saves that share as yours (`sidebar_width_percent`, for every repository), used exactly
/// from then on; the TS saved a width per repository.
pub fn resize_sidebar(app: &mut App, steps: i64) {
    let cols = i64::from(app.browse.terminal_size.cols).max(1);
    let shown = (f64::from(app.layout().sidebar_width) * 100.0 / (cols as f64)).round();
    let next = normalize_sidebar_percent(Some(shown + (steps as f64) * SIDEBAR_PERCENT_STEP));
    app.browse.sidebar_percent = Some(next);
    app.set_status(Some(format!(
        "Sidebar {next}% (saved) · = fits it to the sessions again"
    )));
    save_sidebar_percent(app, Some(next));
}

/// Not in the TS: `=` forgets your sidebar share: it is fitted to the sessions again.
pub fn fit_sidebar(app: &mut App) {
    app.browse.sidebar_percent = None;
    let derived = app.derived();
    measure_sidebar(app, &derived);
    app.set_status(Some("Sidebar fitted to the sessions".to_owned()));
    save_sidebar_percent(app, None);
}

/// `sidebar_width_percent` saved (`None`: removed).
fn save_sidebar_percent(app: &mut App, percent: Option<f64>) {
    let saved = update_app_config(move |config| {
        Ok(AppConfig {
            sidebar_width_percent: percent.map(JsNumber),
            ..config
        })
    });
    drop(app.dispatch().spawn(saved, |app: &mut App, result| {
        if let Err(error) = result {
            app.set_error(Some(error.to_string()));
        }
    }));
}

/// TS `stepIntoPane` (→ / l): type to the agent in its Preview, browse the changes, edit the notes.
pub fn step_into_pane(app: &mut App) {
    let Some(session) = app.selected_session() else {
        return;
    };
    match app.browse.active_tab {
        RightPaneTab::Preview => {
            if session.status == SessionStatus::Running {
                app.preview_focus.session_id = Some(session.id.clone());
                app.preview_focus.in_paste = false;
                // The plain text shows until the watch brings this focus's first styled screen.
                app.panes.preview.screen = None;
                app.set_mode(Mode::PreviewFocus);
            } else {
                app.set_status(Some(
                    "The agent is not running, so there is nothing to type to: s resumes it".to_owned(),
                ));
            }
        }
        RightPaneTab::Git => {
            if workspace_of(&session).is_none() {
                app.set_error(Some(format!("Git is unavailable: {}", no_workspace_reason(&session))));
            } else if app.panes.changes.session_id.as_deref() == Some(session.id.as_str())
                && truthy(app.panes.changes.workspace.as_ref())
            {
                app.set_mode(Mode::ChangesFocus);
            } else {
                app.set_error(Some("Changes are still loading; try again in a moment".to_owned()));
            }
        }
        RightPaneTab::Notes => {
            app.notes_flow_call(|flow, props, _cx| flow.focus(props));
            app.set_mode(Mode::NotesFocus);
        }
        tab => app.set_status(Some(format!(
            "Nothing to scroll here: Enter opens the {} full screen",
            if tab == RightPaneTab::Dev {
                "Dev command"
            } else {
                "shell"
            }
        ))),
    }
}

/// TS `runDev`: start (after reviewing an untrusted repository devCommand) or stop the worktree's Dev; `back` is
/// where Esc from the review returns.
pub fn run_dev(app: &mut App, back: Option<AppCallback>) {
    let Some(session) = app.selected_session() else {
        return;
    };
    if workspace_of(&session).is_none() {
        app.set_error(Some(format!("Dev is unavailable: {}", no_workspace_reason(&session))));
        return;
    }
    if dev_running(app, &session) {
        app.set_mode(Mode::Browse);
        toggle_dev_selected(app);
        return;
    }
    let resume = Rc::new(|app: &mut App, _project: Option<ProjectInfo>| {
        app.set_mode(Mode::Browse);
        toggle_dev_selected(app);
    });
    // TS `project.effective.devCommand?.trim()`.
    fn fallback(project: &ProjectInfo) -> String {
        js_trim(project.effective.dev_command_text().unwrap_or("")).to_owned()
    }
    let options = ReviewOptions {
        back,
        gate: Some(Rc::new(|project: &ProjectInfo| {
            project.needs_review
                && project
                    .project
                    .config
                    .dev_command
                    .as_ref()
                    .is_some_and(|dev| dev.text() != Some(""))
        })),
        // Skipping starts what applies until trusted: the global (or legacy) Dev command, else the built-in `dev`.
        labels: Some(Rc::new(|project: &ProjectInfo| {
            let fallback = fallback(project);
            ReviewLabels {
                enter: Some(hint_short("enter trust & start", "enter trust")),
                skip: Some(hint_short(
                    &format!(
                        "s start {} instead",
                        if fallback.is_empty() { "dev" } else { &fallback }
                    ),
                    "s start fallback",
                )),
            }
        })),
        purpose: Some(Rc::new(|project: &ProjectInfo| {
            let fallback = fallback(project);
            let what = match project.project.config.dev_processes() {
                Some(processes) => format!(
                    "Dev processes: {}",
                    processes
                        .iter()
                        .map(|(name, command)| format!("{name} ({command})"))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                None => format!(
                    "devCommand: {}",
                    project.project.config.dev_command_text().unwrap_or("undefined")
                ),
            };
            format!(
                "About to start Dev with the repository {what} (until trusted, Dev runs {})",
                if fallback.is_empty() {
                    "the built-in fallback dev".to_owned()
                } else {
                    format!("the global {fallback}")
                }
            )
        })),
        ..ReviewOptions::default()
    };
    let cwd = session.cwd.clone();
    app.review_then(&cwd, Some(resume), options);
}

/// TS `toggleDevSelected`: start or stop the selected workspace's Dev.
pub fn toggle_dev_selected(app: &mut App) {
    let (Some(client), Some(session)) = (app.client(), app.selected_session()) else {
        return;
    };
    if workspace_of(&session).is_none() {
        return;
    }
    app.set_busy(true);
    app.set_error(None);
    // Dev is shared by every session in the selected session's workspace; starting/stopping acts on that one process.
    if dev_running(app, &session) {
        let reply = client.stop_dev(&session.id);
        drop(app.dispatch().spawn(reply, move |app: &mut App, result| {
            match result {
                Ok(()) => {
                    app.panes.dev = crate::app::state::empty_pane();
                    app.panes.dev.session_id = Some(session.id.clone());
                    app.panes.dev.cwd = Some(session.cwd.clone());
                    upsert_dev_running(app, &session, false);
                    app.set_status(Some("Stopped Dev".to_owned()));
                }
                Err(error) => app.set_error(Some(error.to_string())),
            }
            app.set_busy(false);
        }));
        return;
    }
    let layout = app.layout();
    let elsewhere = dev_elsewhere(app, &session);
    let updates = app.panes.dev_updates;
    let reply = client.start_dev(
        &session.id,
        i64::from(layout.preview_cols),
        i64::from(layout.preview_rows),
    );
    drop(app.dispatch().spawn(reply, move |app: &mut App, result| {
        match result {
            Ok(next) => {
                let live = next.live;
                let command = match dev_processes(&next) {
                    Some(processes) => Some(
                        processes
                            .iter()
                            .map(|process| process.name.as_str())
                            .collect::<Vec<_>>()
                            .join(" + "),
                    ),
                    None => next.command.clone().filter(|command| !command.is_empty()),
                };
                let live = keep_newer_dev(app, next, updates).unwrap_or(live);
                upsert_dev_running(app, &session, live);
                app.set_active_tab(RightPaneTab::Dev);
                // d on the Dev tab starts it, so a quick second d says plainly what it did.
                app.set_status(Some(format!(
                    "Started Dev{}{} · d d stops it",
                    command.map(|command| format!(": {command}")).unwrap_or_default(),
                    stopped_elsewhere(&elsewhere)
                )));
            }
            Err(error) => app.set_error(Some(error.to_string())),
        }
        app.set_busy(false);
    }));
}

/// Not in the TS: a Dev start reply's record, unless a `dev-updated` arrived after the request (`updates` was the count
/// then): the reply is the snapshot from when Dev started, the update is newer, and a quick command may already have
/// exited with its output. Returns the newer record's `live` when the reply is dropped.
pub(crate) fn keep_newer_dev(app: &mut App, reply: DevRecord, updates: u64) -> Option<bool> {
    if app.panes.dev_updates == updates {
        app.panes.dev = reply;
        return None;
    }
    Some(app.panes.dev.live)
}

/// Not in the TS: the sessions (one per worktree) whose Dev the daemon stops when Dev starts for `session`: Dev runs
/// in one worktree of a repository at a time.
pub(crate) fn dev_elsewhere(app: &App, session: &SessionRecord) -> Vec<String> {
    let here = workspace_of(session);
    let mut seen: Vec<String> = Vec::new();
    let mut titles = Vec::new();
    for other in &app.data.repo_sessions {
        let Some(key) = workspace_of(other).filter(|key| Some(key) != here.as_ref()) else {
            continue;
        };
        if other.dev_running == Some(true) && !seen.contains(&key) {
            seen.push(key);
            titles.push(display_session_title(other, &app.data.repo_sessions));
        }
    }
    titles
}

/// ` · stopped Dev in <titles>` (nothing when none was stopped).
pub(crate) fn stopped_elsewhere(titles: &[String]) -> String {
    if titles.is_empty() {
        String::new()
    } else {
        format!(" · stopped it in {}", titles.join(", "))
    }
}

/// TS: `setSessions(current => upsertSession(current, {...(current.find(...) ?? selectedSession), devRunning}))`.
pub(crate) fn upsert_dev_running(app: &mut App, selected: &SessionRecord, running: bool) {
    let mut record = app
        .data
        .repo_sessions
        .iter()
        .find(|session| session.id == selected.id)
        .cloned()
        .unwrap_or_else(|| selected.clone());
    record.dev_running = Some(running);
    app.upsert(record);
}

/// TS `openInEditor(target, setError)`: Cursor / VS Code; its errors go to the footer. The editor's label when
/// started.
fn open_editor(app: &mut App, target: &str) -> Option<String> {
    let failure: RefCell<Option<String>> = RefCell::new(None);
    let label = open_in_editor(
        target,
        &|message: String| {
            failure.replace(Some(message));
        },
        None,
    );
    if let Some(message) = failure.into_inner() {
        app.set_error(Some(message));
    }
    label
}

/// TS `openSelectedInEditor` (E off Notes): the session's folder in Cursor / VS Code.
pub fn open_selected_in_editor(app: &mut App) {
    let Some(session) = app.selected_session() else {
        app.set_error(Some("no session selected".to_owned()));
        return;
    };
    let target = session
        .worktree
        .as_ref()
        .and_then(|worktree| worktree.path.clone())
        .unwrap_or_else(|| session.cwd.clone());
    if let Some(label) = open_editor(app, &target) {
        app.set_status(Some(format!("Opened {target} in {label}")));
    }
}

/// TS `toggleSelectedDone` (Space).
pub fn toggle_selected_done(app: &mut App) {
    let (Some(client), Some(session)) = (app.client(), app.selected_session()) else {
        return;
    };
    let done = !truthy(session.done_at.as_ref());
    app.set_error(None);
    let reply = client.set_session_done(&session.id, done);
    drop(app.dispatch().spawn(reply, move |app: &mut App, result| match result {
        Ok(updated) => {
            // TS sessionsRef.current: the sessions of the last render, before this update.
            let title = display_session_title(&updated, &app.sessions());
            app.upsert(updated);
            app.set_status(Some(if done {
                format!("Marked done: {title}")
            } else {
                format!("No longer done: {title}")
            }));
        }
        Err(error) => app.set_error(Some(error.to_string())),
    }));
}

/// Not in the TS: U marks the reply that ended the selected session's last turn unread, or read again (the daemon
/// keeps it).
pub fn toggle_selected_reply_read(app: &mut App) {
    let (Some(client), Some(session)) = (app.client(), app.selected_session()) else {
        return;
    };
    let ended = session
        .attention
        .as_ref()
        .is_some_and(|attention| attention.state == AttentionState::ResponseEnded);
    if !ended {
        app.set_status(Some(
            "No reply to mark unread: its agent has not finished a turn".to_owned(),
        ));
        return;
    }
    let read = !session_has_unread_reply(&session);
    app.set_error(None);
    let reply = client.set_reply_read(&session.id, !read);
    drop(app.dispatch().spawn(reply, move |app: &mut App, result| match result {
        Ok(updated) => {
            let title = display_session_title(&updated, &app.sessions());
            app.upsert(updated);
            app.set_status(Some(if read {
                format!("Marked unread: {title}")
            } else {
                format!("Marked read: {title}")
            }));
        }
        Err(error) => app.set_error(Some(error.to_string())),
    }));
}

/// TS `markSelectedMerged` (M).
pub fn mark_selected_merged(app: &mut App) {
    let (Some(client), Some(session)) = (app.client(), app.selected_session()) else {
        return;
    };
    let was_merged = confirm::has_merged_marker(Some(&session));
    app.set_busy(true);
    app.set_error(None);
    let reply = client.mark_session_merged(&session.id, &app.props.cwd);
    drop(app.dispatch().spawn(reply, move |app: &mut App, result| {
        match result {
            Ok(updated) => {
                let status = if was_merged {
                    "Unmarked".to_owned()
                } else {
                    format!(
                        "Marked into {}",
                        confirm::merged_target_branch(&updated).unwrap_or("target branch")
                    )
                };
                app.upsert(updated);
                app.set_status(Some(status));
            }
            Err(error) => app.set_error(Some(error.to_string())),
        }
        app.set_busy(false);
    }));
}

/// TS `restartSelected` (s / S; `project_fingerprint`: the reviewed-and-skipped config).
pub fn restart_selected(app: &mut App, mode: RestartMode, project_fingerprint: Option<String>) {
    let (Some(client), Some(session)) = (app.client(), app.selected_session()) else {
        return;
    };
    if session.status != SessionStatus::Exited {
        return;
    }
    if session
        .worktree
        .as_ref()
        .is_some_and(|worktree| truthy(worktree.deleted_at.as_ref()))
    {
        app.set_error(Some(
            "cannot restart session because its worktree was deleted".to_owned(),
        ));
        return;
    }
    app.set_busy(true);
    app.set_error(None);
    let layout = app.layout();
    let reply = client.restart_session(
        &session.id,
        i64::from(layout.preview_cols),
        i64::from(layout.preview_rows),
        Some(mode),
        project_fingerprint.as_deref(),
    );
    drop(app.dispatch().spawn(reply, |app: &mut App, result| {
        match result {
            Ok(restarted) => {
                app.set_selected_id(Some(restarted.id.clone()));
                app.upsert(restarted);
            }
            Err(error) => app.set_error(Some(error.to_string())),
        }
        app.set_busy(false);
    }));
}

/// TS `refreshSessions` (R).
pub fn refresh_sessions(app: &mut App) {
    let Some(client) = app.client() else {
        app.set_error(Some("still connecting to daemon".to_owned()));
        return;
    };
    let reply = client.subscribe(&app.props.repo_root);
    drop(app.dispatch().spawn(reply, |app: &mut App, result| match result {
        Ok(latest) => app.data.repo_sessions = sort_sessions(latest),
        Err(error) => app.set_error(Some(error.to_string())),
    }));
}

/// TS: A: archive or unarchive the selected session.
pub fn toggle_archive(app: &mut App) {
    let (Some(client), Some(session)) = (app.client(), app.selected_session()) else {
        return;
    };
    let archived = truthy(session.archived_at.as_ref());
    let title = display_session_title(&session, &app.sessions());
    // Archiving hides the session from the default view, so say where it went.
    let status = if archived {
        format!("Unarchived {title}")
    } else {
        format!(
            "Archived {title}{}",
            if app.browse.session_filter == SessionFilter::Active {
                " · hidden here, f A shows archived sessions"
            } else {
                ""
            }
        )
    };
    let reply = client.archive_session(&session.id, !archived);
    drop(app.dispatch().spawn(reply, move |app: &mut App, result| match result {
        Ok(_) => app.set_status(Some(status)),
        Err(error) => app.set_error(Some(error.to_string())),
    }));
}

/// TS: !: the next session that needs attention (blocked ones first, then unread replies).
pub fn next_attention(app: &mut App) {
    let sessions = app.sessions();
    let waiting: Vec<&SessionRecord> = sessions
        .iter()
        .filter(|session| !truthy(session.archived_at.as_ref()) && session_needs_attention(session))
        .collect();
    let targets: Vec<&SessionRecord> = waiting
        .iter()
        .copied()
        .filter(|session| session_blocked(session))
        .chain(waiting.iter().copied().filter(|session| !session_blocked(session)))
        .collect();
    let current = targets
        .iter()
        .position(|session| Some(&session.id) == app.browse.selected_id.as_ref());
    let next = current.map_or(0, |index| index + 1) % targets.len().max(1);
    match targets.get(next) {
        Some(next) => {
            let id = next.id.clone();
            app.browse.session_filter = SessionFilter::Attention;
            app.browse.session_query.clear();
            app.set_selected_id(Some(id));
        }
        None => app.set_status(Some("No known attention requests".to_owned())),
    }
}

/// TS: d (and d d): the Dev tab, a quick double-tap starts or stops Dev.
pub fn dev_tap(app: &mut App, input: &str) {
    let now = now_ms();
    let double_tap = input == "dd" || now - app.browse.last_dev_tap < DEV_DOUBLE_TAP_MS;
    app.browse.last_dev_tap = if double_tap { 0 } else { now };
    show_tab(app, RightPaneTab::Dev);
    if double_tap {
        run_dev(app, None);
    }
}

/// TS: Enter / o: open what the tab shows full screen (on Notes, Enter edits them here).
pub fn open_selected(app: &mut App, input: &Input) {
    let opening = input.input == "o" || input.key.return_;
    let Some(session) = app.selected_session().filter(|_| opening) else {
        return;
    };
    let tab = app.browse.active_tab;
    if tab == RightPaneTab::Notes {
        app.notes_flow_call(|flow, props, _cx| flow.focus(props));
        app.set_mode(Mode::NotesFocus);
        return;
    }
    let workspace_tab = matches!(tab, RightPaneTab::Terminal | RightPaneTab::Git | RightPaneTab::Dev);
    if !(session.status == SessionStatus::Running || workspace_tab) {
        app.set_status(Some(
            "The agent is not running: s resumes it, S starts it fresh".to_owned(),
        ));
        return;
    }
    let pane_name = |target: AttachTarget| match target {
        AttachTarget::Git => "Git",
        AttachTarget::Terminal => "Terminal",
        _ => "Dev",
    };
    if workspace_tab && workspace_of(&session).is_none() {
        let name = match tab {
            RightPaneTab::Git => "Git",
            RightPaneTab::Terminal => "Terminal",
            _ => "Dev",
        };
        app.set_error(Some(format!(
            "{name} is unavailable: {}",
            no_workspace_reason(&session)
        )));
        return;
    }
    if tab == RightPaneTab::Dev && !dev_running(app, &session) {
        app.set_error(Some(
            "Dev is not running: d d starts it (press d twice quickly)".to_owned(),
        ));
        return;
    }
    let target = app.active_attach_target();
    if target == AttachTarget::Action && !app.panes.action.live {
        app.set_error(Some(format!(
            "{} has finished; r runs an action again (v shows the shell)",
            app.panes.action.name.as_deref().unwrap_or("The action")
        )));
        return;
    }
    if !app.active_pane_ready_for_attach() {
        app.set_error(Some(format!(
            "{} tab is still loading; wait for it to appear before attaching",
            pane_name(target)
        )));
        return;
    }
    app.attach_to(&session, target);
}

/// TS: Backspace on an exited session: archive it, or on an archived one ask to remove it for good.
pub fn archive_or_remove(app: &mut App) {
    let Some(session) = app.selected_session() else {
        return;
    };
    if truthy(session.archived_at.as_ref()) {
        confirm::request_remove(app);
        return;
    }
    let Some(client) = app.client() else {
        return;
    };
    let title = display_session_title(&session, &app.sessions());
    let status = format!(
        "Archived {title}{}",
        if app.browse.session_filter == SessionFilter::Active {
            " · f A shows archived sessions, where backspace removes one for good"
        } else {
            ""
        }
    );
    let reply = client.archive_session(&session.id, true);
    drop(app.dispatch().spawn(reply, move |app: &mut App, result| match result {
        Ok(_) => app.set_status(Some(status)),
        Err(error) => app.set_error(Some(error.to_string())),
    }));
}

#[cfg(test)]
pub(crate) mod tests {
    use std::rc::Rc;

    use serde_json::{Value, json};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::UnixStream;
    use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
    use tokio::sync::mpsc::UnboundedReceiver;

    use deckhand_client::live::{LiveClient, LiveClientHandlers};
    use deckhand_core::key::Key;
    use deckhand_protocol::data::{
        AgentActivityStatus, AttachTarget, ProgramKey, SessionAttention, SessionWorktreeRecord, UiExitResult,
    };
    use indexmap::IndexMap;

    use super::*;
    use crate::app::event_loop::{UiModel, Update};
    use crate::app::state::AppProps;
    use crate::app::test_support::{app, session};
    use crate::keymap::{undocumented_in_guide, unknown_topics};

    /// Runs `body` inside a LocalSet (requests and timers are local tasks).
    pub(crate) fn local(body: impl std::future::Future<Output = ()>) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap_or_else(|error| panic!("{error}"));
        tokio::task::LocalSet::new().block_on(&runtime, body);
    }

    /// The daemon end of a connected app's socket.
    pub(crate) struct Daemon {
        lines: BufReader<OwnedReadHalf>,
        writer: OwnedWriteHalf,
    }

    impl Daemon {
        /// Connects `app` to a fake daemon (inside a LocalSet).
        pub(crate) fn connect(app: &mut App) -> Daemon {
            let (client, server) = UnixStream::pair().unwrap_or_else(|error| panic!("{error}"));
            app.connection.client = Some(Rc::new(LiveClient::new(client, LiveClientHandlers::default())));
            let (read, writer) = server.into_split();
            Daemon {
                lines: BufReader::new(read),
                writer,
            }
        }

        /// The next message the app sent.
        pub(crate) async fn next(&mut self) -> Value {
            let mut line = String::new();
            self.lines
                .read_line(&mut line)
                .await
                .unwrap_or_else(|error| panic!("{error}"));
            serde_json::from_str(&line).unwrap_or_else(|error| panic!("{error}: {line}"))
        }

        /// Answers `request` with `data`.
        pub(crate) async fn ok(&mut self, request: &Value, data: Value) {
            let message = json!({"type": "response", "requestId": request["requestId"], "ok": true, "data": data});
            self.send(&message).await;
        }

        /// Fails `request` with `error`.
        pub(crate) async fn fail(&mut self, request: &Value, error: &str) {
            let message = json!({"type": "response", "requestId": request["requestId"], "ok": false, "error": error});
            self.send(&message).await;
        }

        async fn send(&mut self, message: &Value) {
            self.writer
                .write_all(format!("{message}\n").as_bytes())
                .await
                .unwrap_or_else(|error| panic!("{error}"));
        }
    }

    /// Applies the next update sent to the app.
    pub(crate) async fn apply(app: &mut App, rx: &mut UnboundedReceiver<Update<App>>) {
        let update = rx.recv().await.unwrap_or_else(|| panic!("an update"));
        update(app);
    }

    pub(crate) fn text(value: &str) -> Input {
        Input::text(value)
    }

    pub(crate) fn named(key: Key) -> Input {
        Input::key("", key)
    }

    fn json_of(session: &SessionRecord) -> Value {
        serde_json::to_value(session).unwrap_or_else(|error| panic!("{error}"))
    }

    fn attention(state: AttentionState) -> SessionAttention {
        serde_json::from_value(json!({"state": state.as_str(), "event": "e", "at": "2026-01-01T00:00:00.000Z"}))
            .unwrap_or_else(|error| panic!("{error}"))
    }

    fn worktree(path: &str) -> SessionWorktreeRecord {
        let mut worktree = SessionWorktreeRecord::new(SessionWorktreeMode::Managed);
        worktree.path = Some(path.to_owned());
        worktree.branch = Some("feature".to_owned());
        worktree
    }

    fn ids(app: &App) -> Vec<String> {
        app.visible_sessions().into_iter().map(|session| session.id).collect()
    }

    /// An app with `count` running sessions s0..s{count-1}, the first listed selected.
    pub(crate) fn with_sessions(count: usize) -> (App, UnboundedReceiver<Update<App>>) {
        let (mut app, rx) = app(AppProps::default());
        app.data.repo_sessions = (0..count)
            .map(|index| {
                session(
                    &format!("s{index}"),
                    &format!("Session {index}"),
                    SessionStatus::Running,
                )
            })
            .collect();
        let first = ids(&app).first().cloned();
        app.set_selected_id(first);
        (app, rx)
    }

    #[test]
    fn the_keymap_is_in_the_guide() {
        assert_eq!(undocumented_in_guide(KEYMAP), Vec::<&str>::new());
        assert_eq!(unknown_topics(KEYMAP), Vec::<&str>::new());
        assert!(KEYMAP.iter().any(|binding| binding.matches(&text("J"))));
    }

    #[test]
    fn tabs_cycle_both_ways_and_reset_the_scroll() {
        let (mut app, _rx) = with_sessions(1);
        app.preview_focus.scroll_offset = 4;
        handle_browse(&mut app, &named(Key { tab: true, ..Key::NONE }));
        assert_eq!(app.browse.active_tab, RightPaneTab::Terminal);
        assert_eq!(app.preview_focus.scroll_offset, 0);
        handle_browse(&mut app, &text("]"));
        assert_eq!(app.browse.active_tab, RightPaneTab::Git);
        handle_browse(
            &mut app,
            &named(Key {
                tab: true,
                shift: true,
                ..Key::NONE
            }),
        );
        assert_eq!(app.browse.active_tab, RightPaneTab::Terminal);
        handle_browse(&mut app, &text("["));
        handle_browse(&mut app, &text("["));
        assert_eq!(app.browse.active_tab, RightPaneTab::Notes);
        handle_browse(&mut app, &text("]"));
        assert_eq!(app.browse.active_tab, RightPaneTab::Preview);
        for (key, tab) in [
            ("t", RightPaneTab::Terminal),
            ("g", RightPaneTab::Git),
            ("a", RightPaneTab::Notes),
            ("p", RightPaneTab::Preview),
        ] {
            app.preview_focus.scroll_offset = 2;
            handle_browse(&mut app, &text(key));
            assert_eq!(app.browse.active_tab, tab);
            assert_eq!(app.preview_focus.scroll_offset, 0);
        }
    }

    #[test]
    fn digits_select_by_number_at_once_up_to_ten_sessions() {
        let (mut app, _rx) = with_sessions(10);
        let order = ids(&app);
        handle_browse(&mut app, &text("3"));
        assert_eq!(app.browse.selected_id.as_ref(), order.get(2));
        handle_browse(&mut app, &text("0"));
        assert_eq!(app.browse.selected_id.as_ref(), order.get(9));
        assert_eq!(app.browse.numeric_selection, "");
        app.data.repo_sessions.truncate(4);
        handle_browse(&mut app, &text("7"));
        assert_eq!(app.messages.error.as_deref(), Some("no visible session 7"));
        handle_browse(&mut app, &text("1"));
        assert_eq!(app.messages.error, None);
    }

    #[test]
    fn digits_are_typed_with_more_than_ten_sessions() {
        let (mut app, _rx) = with_sessions(12);
        let order = ids(&app);
        handle_browse(&mut app, &text("1"));
        assert_eq!(app.browse.numeric_selection, "1");
        handle_browse(&mut app, &text("2"));
        assert_eq!(app.browse.numeric_selection, "12");
        handle_browse(
            &mut app,
            &named(Key {
                return_: true,
                ..Key::NONE
            }),
        );
        assert_eq!(app.browse.selected_id.as_ref(), order.get(11));
        assert_eq!(app.browse.numeric_selection, "");
        handle_browse(&mut app, &text("4"));
        handle_browse(
            &mut app,
            &named(Key {
                backspace: true,
                ..Key::NONE
            }),
        );
        assert_eq!(app.browse.numeric_selection, "");
        handle_browse(&mut app, &text("9"));
        handle_browse(&mut app, &text("9"));
        handle_browse(
            &mut app,
            &named(Key {
                escape: true,
                ..Key::NONE
            }),
        );
        assert_eq!(app.browse.numeric_selection, "");
        // Another key drops the number and does its own thing.
        app.set_selected_id(order.first().cloned());
        handle_browse(&mut app, &text("5"));
        handle_browse(&mut app, &text("j"));
        assert_eq!(app.browse.numeric_selection, "");
        assert_eq!(app.browse.selected_id.as_ref(), order.get(1));
        confirm_numeric_selection(&mut app, "13");
        assert_eq!(app.messages.error.as_deref(), Some("no visible session 13"));
        confirm_numeric_selection(&mut app, "0");
        assert_eq!(app.messages.error.as_deref(), Some("no visible session 0"));
        confirm_numeric_selection(&mut app, &"9".repeat(60));
        assert_eq!(
            app.messages.error,
            Some(format!("no visible session {}", "9".repeat(60)))
        );
        app.set_error(None);
        confirm_numeric_selection(&mut app, "");
        assert_eq!(app.messages.error, None);
    }

    #[test]
    fn j_and_k_wrap_around_the_visible_list() {
        let (mut app, _rx) = with_sessions(3);
        let order = ids(&app);
        handle_browse(&mut app, &text("k"));
        assert_eq!(app.browse.selected_id.as_ref(), order.get(2));
        handle_browse(&mut app, &text("j"));
        assert_eq!(app.browse.selected_id.as_ref(), order.first());
        handle_browse(
            &mut app,
            &named(Key {
                down_arrow: true,
                ..Key::NONE
            }),
        );
        assert_eq!(app.browse.selected_id.as_ref(), order.get(1));
        handle_browse(
            &mut app,
            &named(Key {
                up_arrow: true,
                ..Key::NONE
            }),
        );
        assert_eq!(app.browse.selected_id.as_ref(), order.first());
        // A hidden selection counts as the first row.
        app.set_selected_id(Some("gone".to_owned()));
        move_selection(&mut app, 1);
        assert_eq!(app.browse.selected_id.as_ref(), order.get(1));
        app.data.repo_sessions.clear();
        move_selection(&mut app, 1);
        assert_eq!(app.browse.selected_id.as_ref(), order.get(1));
    }

    #[test]
    fn c_hides_exited_children_then_collapses_then_expands() {
        let (mut app, _rx) = app(AppProps::default());
        let parent = session("p", "Parent", SessionStatus::Running);
        let mut live = session("c1", "Live child", SessionStatus::Running);
        live.parent_session_id = Some("p".to_owned());
        let mut done = session("c2", "Done child", SessionStatus::Exited);
        done.parent_session_id = Some("p".to_owned());
        let lone = session("l", "Lone", SessionStatus::Running);
        app.data.repo_sessions = vec![parent, live, done, lone];
        app.set_selected_id(Some("l".to_owned()));
        handle_browse(&mut app, &text("c"));
        assert_eq!(
            app.messages.error.as_deref(),
            Some("selected session has no sub-sessions")
        );
        app.set_selected_id(Some("p".to_owned()));
        handle_browse(&mut app, &text("c"));
        assert_eq!(app.messages.error, None);
        assert_eq!(
            app.messages.status_message.as_deref(),
            Some("collapsed 1 exited sub-session")
        );
        assert!(app.browse.hidden_exited_session_ids.contains("c2"));
        handle_browse(&mut app, &text("c"));
        assert_eq!(
            app.messages.status_message.as_deref(),
            Some("collapsed all sub-sessions")
        );
        assert!(app.browse.collapsed_session_ids.contains("p"));
        handle_browse(&mut app, &text("c"));
        assert_eq!(
            app.messages.status_message.as_deref(),
            Some("expanded all sub-sessions")
        );
        assert!(app.browse.collapsed_session_ids.is_empty());
        assert!(app.browse.hidden_exited_session_ids.is_empty());
    }

    #[test]
    fn the_sidebar_share_steps_by_two_percent_and_is_saved() {
        let _guard = crate::app::modes::preview_focus::tests::ENV
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let home = tempfile::Builder::new()
            .prefix("deckhand-sidebar-")
            .tempdir()
            .unwrap_or_else(|error| panic!("{error}"));
        let previous = std::env::var_os(deckhand_sys::paths::HOME_ENV);
        // SAFETY: tests touching the environment hold ENV.
        unsafe { std::env::set_var(deckhand_sys::paths::HOME_ENV, home.path()) };
        local(async {
            let (mut app, mut rx) = app(AppProps::default());
            // Fitted (nothing chosen): 30% of 130 columns with nothing listed.
            app.browse.sidebar_percent = None;
            assert_eq!(app.layout().sidebar_width, 39);
            // > moves it 2% from where it is and saves that share as yours.
            handle_browse(&mut app, &text(">"));
            assert_eq!(app.browse.sidebar_percent, Some(32.0));
            assert_eq!(app.layout().sidebar_width, 41);
            // A fitted sidebar narrower than 30% widens from its own width, not from 30%.
            app.browse.sidebar_percent = None;
            app.browse.sidebar_content = Some(26);
            handle_browse(&mut app, &text(">"));
            assert_eq!(app.browse.sidebar_percent, Some(22.0));
            // Yours is used exactly, even below what the titles need.
            handle_browse(&mut app, &text("<"));
            handle_browse(&mut app, &text("<"));
            assert_eq!(app.browse.sidebar_percent, Some(18.0));
            assert_eq!(app.layout().sidebar_width, 24);
            // One save per press.
            for _ in 0..4 {
                apply(&mut app, &mut rx).await;
            }
            let config = deckhand_sys::storage::load_app_config()
                .await
                .unwrap_or_else(|error| panic!("{error}"));
            assert_eq!(config.sidebar_width_percent.map(|value| value.0), Some(18.0));
            // = forgets it.
            handle_browse(&mut app, &text("="));
            assert_eq!(app.browse.sidebar_percent, None);
            apply(&mut app, &mut rx).await;
            let config = deckhand_sys::storage::load_app_config()
                .await
                .unwrap_or_else(|error| panic!("{error}"));
            assert_eq!(config.sidebar_width_percent, None);
        });
        // SAFETY: as above (ENV is still held).
        unsafe {
            match previous {
                Some(value) => std::env::set_var(deckhand_sys::paths::HOME_ENV, value),
                None => std::env::remove_var(deckhand_sys::paths::HOME_ENV),
            }
        }
    }

    #[test]
    fn stepping_into_the_pane_depends_on_the_tab() {
        let (mut app, _rx) = with_sessions(1);
        handle_browse(&mut app, &text("l"));
        assert_eq!(app.mode, Mode::PreviewFocus);
        app.set_mode(Mode::Browse);
        app.data.repo_sessions[0].status = SessionStatus::Exited;
        handle_browse(
            &mut app,
            &named(Key {
                right_arrow: true,
                ..Key::NONE
            }),
        );
        assert_eq!(app.mode, Mode::Browse);
        assert_eq!(
            app.messages.status_message.as_deref(),
            Some("The agent is not running, so there is nothing to type to: s resumes it")
        );
        app.set_active_tab(RightPaneTab::Git);
        step_into_pane(&mut app);
        assert_eq!(
            app.messages.error.as_deref(),
            Some("Changes are still loading; try again in a moment")
        );
        app.panes.changes.session_id = Some("s0".to_owned());
        app.panes.changes.workspace = Some("/repo".to_owned());
        step_into_pane(&mut app);
        assert_eq!(app.mode, Mode::ChangesFocus);
        app.set_mode(Mode::Browse);
        app.data.repo_sessions[0].requested_worktree_mode = Some(WorktreeMode::New);
        let reason = no_workspace_reason(&app.data.repo_sessions[0]);
        step_into_pane(&mut app);
        assert_eq!(app.messages.error, Some(format!("Git is unavailable: {reason}")));
        app.set_active_tab(RightPaneTab::Terminal);
        step_into_pane(&mut app);
        assert_eq!(
            app.messages.status_message.as_deref(),
            Some("Nothing to scroll here: Enter opens the shell full screen")
        );
        app.set_active_tab(RightPaneTab::Dev);
        step_into_pane(&mut app);
        assert_eq!(
            app.messages.status_message.as_deref(),
            Some("Nothing to scroll here: Enter opens the Dev command full screen")
        );
        // h / ← are already out of the pane.
        handle_browse(&mut app, &text("h"));
        assert_eq!(app.mode, Mode::Browse);
    }

    #[test]
    fn simple_doors_quit_help_and_search() {
        let (mut app, _rx) = with_sessions(1);
        app.flows.help.open(Some(2));
        handle_browse(&mut app, &text("?"));
        assert_eq!(app.mode, Mode::Help);
        app.set_mode(Mode::Browse);
        handle_browse(&mut app, &text("/"));
        assert_eq!(app.mode, Mode::Search);
        app.set_mode(Mode::Browse);
        // Without a daemon: W and R say so; C, G, A, i, H do nothing.
        handle_browse(&mut app, &text("W"));
        assert_eq!(app.messages.error.as_deref(), Some("still connecting to daemon"));
        assert_eq!(app.mode, Mode::Browse);
        app.set_error(None);
        handle_browse(&mut app, &text("R"));
        assert_eq!(app.messages.error.as_deref(), Some("still connecting to daemon"));
        app.set_error(None);
        for key in ["C", "G", "A", "i", "H"] {
            handle_browse(&mut app, &text(key));
            assert_eq!(app.mode, Mode::Browse, "{key}");
            assert_eq!(app.messages.error, None, "{key}");
        }
        handle_browse(&mut app, &text("q"));
        assert_eq!(UiModel::exit(&mut app), Some(UiExitResult::Quit));
    }

    #[test]
    fn e_without_a_selection_says_so() {
        let (mut app, _rx) = app(AppProps::default());
        handle_browse(&mut app, &text("E"));
        assert_eq!(app.messages.error.as_deref(), Some("no session selected"));
        // The session keys need a selected session.
        for key in ["m", "M", " ", "x", "X", "s", "S", "o", "v"] {
            app.set_error(None);
            handle_browse(&mut app, &text(key));
            assert_eq!(app.messages.error, None, "{key}");
            assert_eq!(app.messages.status_message, None, "{key}");
        }
        handle_browse(
            &mut app,
            &named(Key {
                return_: true,
                ..Key::NONE
            }),
        );
        assert_eq!(UiModel::exit(&mut app), None);
    }

    #[test]
    fn bang_cycles_blocked_sessions_then_unread_replies() {
        let (mut app, _rx) = app(AppProps::default());
        let mut reply = session("r", "Reply", SessionStatus::Running);
        reply.attention = Some(attention(AttentionState::ResponseEnded));
        let mut asks = session("q", "Asks", SessionStatus::Running);
        asks.attention = Some(attention(AttentionState::NeedsInput));
        let mut archived = session("z", "Archived", SessionStatus::Exited);
        archived.attention = Some(attention(AttentionState::NeedsInput));
        archived.archived_at = Some("2026-01-01T00:00:00.000Z".to_owned());
        let quiet = session("c", "Calm", SessionStatus::Running);
        app.data.repo_sessions = vec![reply, asks, archived, quiet];
        app.browse.session_query = "x".to_owned();
        handle_browse(&mut app, &text("!"));
        assert_eq!(app.browse.selected_id.as_deref(), Some("q"));
        assert_eq!(app.browse.session_filter, SessionFilter::Attention);
        assert_eq!(app.browse.session_query, "");
        handle_browse(&mut app, &text("!"));
        assert_eq!(app.browse.selected_id.as_deref(), Some("r"));
        handle_browse(&mut app, &text("!"));
        assert_eq!(app.browse.selected_id.as_deref(), Some("q"));
        app.data.repo_sessions.retain(|session| session.id == "c");
        handle_browse(&mut app, &text("!"));
        assert_eq!(
            app.messages.status_message.as_deref(),
            Some("No known attention requests")
        );
    }

    #[test]
    fn d_shows_dev_and_a_double_tap_runs_it() {
        let (mut app, _rx) = with_sessions(1);
        handle_browse(&mut app, &text("d"));
        assert_eq!(app.browse.active_tab, RightPaneTab::Dev);
        assert!(app.browse.last_dev_tap > 0);
        assert_eq!(app.messages.error, None);
        // Without a workspace the second d says why.
        app.data.repo_sessions[0].requested_worktree_mode = Some(WorktreeMode::New);
        let reason = no_workspace_reason(&app.data.repo_sessions[0]);
        handle_browse(&mut app, &text("d"));
        assert_eq!(app.browse.last_dev_tap, 0);
        assert_eq!(app.messages.error, Some(format!("Dev is unavailable: {reason}")));
        app.set_error(None);
        // A later second d is a first tap again.
        app.browse.last_dev_tap = 1;
        dev_tap(&mut app, "d");
        assert_eq!(app.messages.error, None);
        dev_tap(&mut app, "dd");
        assert_eq!(app.messages.error, Some(format!("Dev is unavailable: {reason}")));
    }

    #[test]
    fn v_switches_the_terminal_between_shell_and_action() {
        let (mut app, _rx) = with_sessions(1);
        handle_browse(&mut app, &text("v"));
        assert_eq!(app.messages.error, None);
        app.set_active_tab(RightPaneTab::Terminal);
        handle_browse(&mut app, &text("v"));
        assert_eq!(
            app.messages.error.as_deref(),
            Some("No action has run in this worktree yet; r runs one")
        );
        app.panes.action.session_id = Some("s0".to_owned());
        app.panes.action.workspace = Some("/repo".to_owned());
        app.panes.action.command = Some("npm test".to_owned());
        handle_browse(&mut app, &text("v"));
        assert_eq!(app.panes.terminal_view, TerminalView::Action);
        handle_browse(&mut app, &text("v"));
        assert_eq!(app.panes.terminal_view, TerminalView::Shell);
    }

    #[test]
    fn merge_and_stop_keys_explain_what_they_cannot_do() {
        let (mut app, _rx) = with_sessions(1);
        handle_browse(&mut app, &text("m"));
        assert_eq!(
            app.messages.status_message.as_deref(),
            Some("Not in a worktree, so there is nothing to merge. Use Space to mark it done")
        );
        app.data.repo_sessions[0].requested_worktree_mode = Some(WorktreeMode::New);
        handle_browse(&mut app, &text("m"));
        assert_eq!(
            app.messages.status_message.as_deref(),
            Some("Its worktree is not ready yet")
        );
        let mut deleted = worktree("/wt");
        deleted.deleted_at = Some("2026-01-02T00:00:00.000Z".to_owned());
        app.data.repo_sessions[0].worktree = Some(deleted);
        handle_browse(&mut app, &text("m"));
        assert_eq!(
            app.messages.status_message.as_deref(),
            Some("Its worktree was deleted: nothing to merge")
        );
        app.data.repo_sessions[0].status = SessionStatus::Exited;
        handle_browse(&mut app, &text("x"));
        assert_eq!(
            app.messages.status_message.as_deref(),
            Some("Already stopped: Backspace archives it")
        );
        app.data.repo_sessions[0].worktree = None;
        app.data.repo_sessions[0].archived_at = Some("2026-01-02T00:00:00.000Z".to_owned());
        app.browse.session_filter = SessionFilter::All;
        handle_browse(&mut app, &text("X"));
        assert_eq!(
            app.messages.status_message.as_deref(),
            Some("Already stopped: Backspace removes it for good (asking first), s resumes it")
        );
    }

    #[test]
    fn restart_refuses_a_deleted_worktree() {
        local(async {
            let (mut app, _rx) = with_sessions(1);
            let _daemon = Daemon::connect(&mut app);
            let mut deleted = worktree("/wt");
            deleted.deleted_at = Some("2026-01-02T00:00:00.000Z".to_owned());
            app.data.repo_sessions[0].worktree = Some(deleted);
            // Running: s does nothing.
            handle_browse(&mut app, &text("s"));
            assert_eq!(app.messages.error, None);
            app.data.repo_sessions[0].status = SessionStatus::Exited;
            handle_browse(&mut app, &text("s"));
            assert_eq!(
                app.messages.error.as_deref(),
                Some("cannot restart session because its worktree was deleted")
            );
            assert!(!app.messages.busy);
        });
    }

    #[test]
    fn enter_opens_the_tab_or_says_why_not() {
        let (mut app, _rx) = with_sessions(1);
        app.data.repo_sessions[0].status = SessionStatus::Exited;
        let enter = named(Key {
            return_: true,
            ..Key::NONE
        });
        handle_browse(&mut app, &enter);
        assert_eq!(
            app.messages.status_message.as_deref(),
            Some("The agent is not running: s resumes it, S starts it fresh")
        );
        app.set_active_tab(RightPaneTab::Dev);
        handle_browse(&mut app, &enter);
        assert_eq!(
            app.messages.error.as_deref(),
            Some("Dev is not running: d d starts it (press d twice quickly)")
        );
        app.set_active_tab(RightPaneTab::Git);
        handle_browse(&mut app, &text("o"));
        assert_eq!(
            app.messages.error.as_deref(),
            Some("Git tab is still loading; wait for it to appear before attaching")
        );
        app.set_active_tab(RightPaneTab::Terminal);
        app.panes.action.session_id = Some("s0".to_owned());
        app.panes.action.workspace = Some("/repo".to_owned());
        app.panes.action.command = Some("npm test".to_owned());
        app.panes.action.name = Some("test".to_owned());
        app.panes.terminal_view = TerminalView::Action;
        handle_browse(&mut app, &enter);
        assert_eq!(
            app.messages.error.as_deref(),
            Some("test has finished; r runs an action again (v shows the shell)")
        );
        app.panes.terminal_view = TerminalView::Shell;
        handle_browse(&mut app, &enter);
        assert_eq!(
            app.messages.error.as_deref(),
            Some("Terminal tab is still loading; wait for it to appear before attaching")
        );
        app.data.repo_sessions[0].requested_worktree_mode = Some(WorktreeMode::New);
        let reason = no_workspace_reason(&app.data.repo_sessions[0]);
        handle_browse(&mut app, &enter);
        assert_eq!(app.messages.error, Some(format!("Terminal is unavailable: {reason}")));
        app.data.repo_sessions[0].requested_worktree_mode = None;
        app.panes.terminal.session_id = Some("s0".to_owned());
        app.panes.terminal.live = true;
        handle_browse(&mut app, &enter);
        assert!(matches!(
            UiModel::exit(&mut app),
            Some(UiExitResult::Attach {
                target: AttachTarget::Terminal,
                ..
            })
        ));
        // Other keys open nothing.
        open_selected(&mut app, &text("z"));
        assert_eq!(UiModel::exit(&mut app), None);
    }

    #[test]
    fn enter_on_a_running_agent_attaches_to_it() {
        let (mut app, _rx) = with_sessions(1);
        app.data.repo_sessions[0].agent_status = Some(AgentActivityStatus::Active);
        handle_browse(
            &mut app,
            &named(Key {
                return_: true,
                ..Key::NONE
            }),
        );
        assert_eq!(
            UiModel::exit(&mut app),
            Some(UiExitResult::Attach {
                session_id: "s0".to_owned(),
                target: AttachTarget::Agent,
                title: Some("Session 0".to_owned()),
                cwd: Some("/repo".to_owned()),
                program: Some(ProgramKey::Claude),
                process: None,
                status_title: Some("Session 0".to_owned()),
            })
        );
    }

    #[test]
    fn archiving_reports_where_the_session_went() {
        local(async {
            let (mut app, mut rx) = with_sessions(1);
            let mut daemon = Daemon::connect(&mut app);
            handle_browse(&mut app, &text("A"));
            let request = daemon.next().await;
            assert_eq!(request["type"], "archive-session");
            assert_eq!(request["sessionId"], "s0");
            assert_eq!(request["archived"], true);
            let mut archived = app.data.repo_sessions[0].clone();
            archived.archived_at = Some("2026-01-02T00:00:00.000Z".to_owned());
            daemon.ok(&request, json_of(&archived)).await;
            apply(&mut app, &mut rx).await;
            assert_eq!(
                app.messages.status_message.as_deref(),
                Some("Archived Session 0 · hidden here, f A shows archived sessions")
            );
            // Backspace on an exited session archives it too.
            app.data.repo_sessions[0].status = SessionStatus::Exited;
            handle_browse(
                &mut app,
                &named(Key {
                    backspace: true,
                    ..Key::NONE
                }),
            );
            let request = daemon.next().await;
            assert_eq!(request["type"], "archive-session");
            daemon.fail(&request, "no such session").await;
            apply(&mut app, &mut rx).await;
            assert_eq!(app.messages.error.as_deref(), Some("no such session"));
            app.browse.session_filter = SessionFilter::All;
            app.data.repo_sessions[0].status = SessionStatus::Running;
            app.data.repo_sessions[0].archived_at = Some("2026-01-02T00:00:00.000Z".to_owned());
            handle_browse(&mut app, &text("A"));
            let request = daemon.next().await;
            assert_eq!(request["archived"], false);
            daemon.ok(&request, json_of(&archived)).await;
            apply(&mut app, &mut rx).await;
            assert_eq!(app.messages.status_message.as_deref(), Some("Unarchived Session 0"));
            app.data.repo_sessions[0].status = SessionStatus::Exited;
            app.data.repo_sessions[0].archived_at = None;
            handle_browse(
                &mut app,
                &named(Key {
                    delete: true,
                    ..Key::NONE
                }),
            );
            let request = daemon.next().await;
            daemon.ok(&request, json_of(&archived)).await;
            apply(&mut app, &mut rx).await;
            assert_eq!(app.messages.status_message.as_deref(), Some("Archived Session 0"));
        });
    }

    #[test]
    fn u_toggles_an_ended_reply_unread_and_read() {
        local(async {
            let (mut app, mut rx) = with_sessions(1);
            let mut daemon = Daemon::connect(&mut app);
            // Nothing ended yet: nothing to mark, and nothing is sent.
            handle_browse(&mut app, &text("U"));
            assert_eq!(
                app.messages.status_message.as_deref(),
                Some("No reply to mark unread: its agent has not finished a turn")
            );
            let mut session = app.data.repo_sessions[0].clone();
            session.attention = Some(SessionAttention {
                state: AttentionState::ResponseEnded,
                event: "Stop".to_owned(),
                at: "2026-01-02T00:00:00.000Z".to_owned(),
                reason: None,
                extra: IndexMap::new(),
            });
            session.reply_read_at = Some("2026-01-02T00:05:00.000Z".to_owned());
            app.data.repo_sessions[0] = session.clone();
            handle_browse(&mut app, &text("U"));
            let request = daemon.next().await;
            assert_eq!(request["type"], "set-reply-read");
            assert_eq!(request["read"], false);
            session.reply_read_at = None;
            daemon.ok(&request, json_of(&session)).await;
            apply(&mut app, &mut rx).await;
            assert_eq!(app.messages.status_message.as_deref(), Some("Marked unread: Session 0"));
            assert!(session_has_unread_reply(&app.sessions()[0]));
            handle_browse(&mut app, &text("U"));
            let request = daemon.next().await;
            assert_eq!(request["read"], true);
            session.reply_read_at = Some("2026-01-02T00:06:00.000Z".to_owned());
            daemon.ok(&request, json_of(&session)).await;
            apply(&mut app, &mut rx).await;
            assert_eq!(app.messages.status_message.as_deref(), Some("Marked read: Session 0"));
            assert!(!session_has_unread_reply(&app.sessions()[0]));
        });
    }

    #[test]
    fn space_toggles_done_and_says_so() {
        local(async {
            let (mut app, mut rx) = with_sessions(1);
            let mut daemon = Daemon::connect(&mut app);
            handle_browse(&mut app, &text(" "));
            let request = daemon.next().await;
            assert_eq!(request["type"], "set-session-done");
            assert_eq!(request["done"], true);
            let mut done = app.data.repo_sessions[0].clone();
            done.done_at = Some("2026-01-02T00:00:00.000Z".to_owned());
            daemon.ok(&request, json_of(&done)).await;
            apply(&mut app, &mut rx).await;
            assert_eq!(app.messages.status_message.as_deref(), Some("Marked done: Session 0"));
            assert!(app.data.repo_sessions[0].done_at.is_some());
            handle_browse(&mut app, &text(" "));
            let request = daemon.next().await;
            assert_eq!(request["done"], false);
            done.done_at = None;
            daemon.ok(&request, json_of(&done)).await;
            apply(&mut app, &mut rx).await;
            assert_eq!(
                app.messages.status_message.as_deref(),
                Some("No longer done: Session 0")
            );
        });
    }

    #[test]
    fn reorder_refresh_and_restart_go_to_the_daemon() {
        local(async {
            let (mut app, mut rx) = with_sessions(2);
            let mut daemon = Daemon::connect(&mut app);
            handle_browse(&mut app, &text("J"));
            assert!(app.messages.busy);
            let request = daemon.next().await;
            assert_eq!(request["type"], "reorder-session");
            assert_eq!(request["direction"], "down");
            let mut moved = app.data.repo_sessions[0].clone();
            moved.title = "Moved".to_owned();
            daemon.ok(&request, json!([json_of(&moved)])).await;
            apply(&mut app, &mut rx).await;
            assert!(!app.messages.busy);
            assert_eq!(app.data.repo_sessions.len(), 2);
            assert!(app.data.repo_sessions.iter().any(|session| session.title == "Moved"));
            handle_browse(&mut app, &text("K"));
            let request = daemon.next().await;
            assert_eq!(request["direction"], "up");
            daemon.fail(&request, "cannot move").await;
            apply(&mut app, &mut rx).await;
            assert_eq!(app.messages.error.as_deref(), Some("cannot move"));
            assert!(!app.messages.busy);

            handle_browse(&mut app, &text("R"));
            let request = daemon.next().await;
            assert_eq!(request["type"], "subscribe");
            assert_eq!(request["repoRoot"], "/repo");
            let fresh = session("s9", "Fresh", SessionStatus::Running);
            daemon.ok(&request, json!([json_of(&fresh)])).await;
            apply(&mut app, &mut rx).await;
            assert_eq!(app.data.repo_sessions.len(), 1);

            let mut exited = session("s9", "Fresh", SessionStatus::Exited);
            app.data.repo_sessions = vec![exited.clone()];
            app.set_selected_id(Some("s9".to_owned()));
            handle_browse(&mut app, &text("S"));
            let request = daemon.next().await;
            assert_eq!(request["type"], "restart");
            assert_eq!(request["mode"], "fresh");
            assert_eq!(request["cols"], app.layout().preview_cols);
            exited.id = "s10".to_owned();
            exited.status = SessionStatus::Running;
            daemon.ok(&request, json_of(&exited)).await;
            apply(&mut app, &mut rx).await;
            assert_eq!(app.browse.selected_id.as_deref(), Some("s10"));
            assert_eq!(app.data.repo_sessions.len(), 2);
            assert!(!app.messages.busy);
        });
    }

    #[test]
    fn owed_setup_is_reviewed_before_a_restart() {
        local(async {
            let (mut app, mut rx) = with_sessions(1);
            let mut daemon = Daemon::connect(&mut app);
            app.data.repo_sessions[0].status = SessionStatus::Exited;
            app.data.repo_sessions[0].setup = Some(
                serde_json::from_value(json!({"command": "npm i", "state": "failed", "output": ""}))
                    .unwrap_or_else(|error| panic!("{error}")),
            );
            handle_browse(&mut app, &text("s"));
            let request = daemon.next().await;
            assert_eq!(request["type"], "project-info");
            let project = json!({
                "root": "/repo", "trustRoot": "/repo", "path": "/repo/deckhand.json",
                "config": {"setupCommand": "npm i"}, "exists": true, "fingerprint": "fp1",
                "trusted": false, "needsReview": true, "effective": {}
            });
            daemon.ok(&request, project).await;
            apply(&mut app, &mut rx).await;
            assert_eq!(app.mode, Mode::ReviewProject);
            let review = app.review.review.clone().unwrap_or_else(|| panic!("a review"));
            assert_eq!(
                review.purpose.as_deref(),
                Some("About to run the repository setupCommand: npm i")
            );
            let labels = review.labels.clone().unwrap_or_default();
            assert_eq!(
                labels.skip.map(|hint| hint.text).as_deref(),
                Some("s retry without the repo's setup")
            );
            // Going on restarts with the reviewed config's fingerprint.
            let resume = review.resume.clone().unwrap_or_else(|| panic!("resume"));
            app.set_mode(Mode::Browse);
            resume(&mut app, Some(review.project.clone()));
            let request = daemon.next().await;
            assert_eq!(request["type"], "restart");
            assert_eq!(request["mode"], "resume");
            assert_eq!(request["projectFingerprint"], "fp1");
        });
    }

    #[test]
    fn a_double_d_reviews_then_starts_and_stops_dev() {
        local(async {
            let (mut app, mut rx) = with_sessions(1);
            let mut daemon = Daemon::connect(&mut app);
            handle_browse(&mut app, &text("dd"));
            let request = daemon.next().await;
            assert_eq!(request["type"], "project-info");
            let project = json!({
                "root": "/repo", "trustRoot": "/repo", "config": {"devCommand": "npm run dev"}, "exists": true,
                "fingerprint": "fp", "trusted": false, "needsReview": true, "effective": {"devCommand": " vite "}
            });
            daemon.ok(&request, project).await;
            apply(&mut app, &mut rx).await;
            assert_eq!(app.mode, Mode::ReviewProject);
            let review = app.review.review.clone().unwrap_or_else(|| panic!("a review"));
            assert_eq!(
                review.purpose.as_deref(),
                Some(
                    "About to start Dev with the repository devCommand: npm run dev (until trusted, Dev runs the \
                     global vite)"
                )
            );
            let labels = review.labels.clone().unwrap_or_default();
            assert_eq!(
                labels.enter.map(|hint| hint.text).as_deref(),
                Some("enter trust & start")
            );
            assert_eq!(
                labels.skip.map(|hint| hint.text).as_deref(),
                Some("s start vite instead")
            );
            let resume = review.resume.clone().unwrap_or_else(|| panic!("resume"));
            resume(&mut app, Some(review.project.clone()));
            assert_eq!(app.mode, Mode::Browse);
            assert!(app.messages.busy);
            let request = daemon.next().await;
            assert_eq!(request["type"], "start-dev");
            daemon
                .ok(
                    &request,
                    json!({"content": "", "live": true, "command": "npm run dev", "sessionId": "s0"}),
                )
                .await;
            apply(&mut app, &mut rx).await;
            assert!(!app.messages.busy);
            assert_eq!(app.browse.active_tab, RightPaneTab::Dev);
            assert_eq!(app.data.repo_sessions[0].dev_running, Some(true));
            assert_eq!(
                app.messages.status_message.as_deref(),
                Some("Started Dev: npm run dev · d d stops it")
            );
            // Running: d d stops it without a review.
            handle_browse(&mut app, &text("dd"));
            let request = daemon.next().await;
            assert_eq!(request["type"], "stop-dev");
            daemon.ok(&request, Value::Null).await;
            apply(&mut app, &mut rx).await;
            assert_eq!(app.messages.status_message.as_deref(), Some("Stopped Dev"));
            assert_eq!(app.data.repo_sessions[0].dev_running, Some(false));
            assert!(!app.panes.dev.live);
            assert_eq!(app.panes.dev.session_id.as_deref(), Some("s0"));
            assert_eq!(app.panes.dev.cwd.as_deref(), Some("/repo"));
        });
    }

    #[test]
    fn a_dev_update_newer_than_the_start_reply_is_kept() {
        // A quick command prints and exits before the start reply arrives. The reply is the snapshot from when Dev
        // started, so it must not replace the newer output with an empty, live pane.
        local(async {
            let (mut app, mut rx) = with_sessions(1);
            let mut daemon = Daemon::connect(&mut app);
            handle_browse(&mut app, &text("dd"));
            let request = daemon.next().await;
            assert_eq!(request["type"], "project-info");
            let project = json!({
                "root": "/repo", "trustRoot": "/repo", "config": {"devCommand": "echo hi"}, "exists": true,
                "fingerprint": "fp", "trusted": true, "needsReview": false, "effective": {"devCommand": "echo hi"}
            });
            daemon.ok(&request, project).await;
            apply(&mut app, &mut rx).await;
            let request = daemon.next().await;
            assert_eq!(request["type"], "start-dev");
            let exited = json!({"content": "hi", "live": false, "command": "echo hi", "sessionId": "s0"});
            crate::app::effects::apply_dev_update(
                &mut app,
                serde_json::from_value(exited).unwrap_or_else(|error| panic!("{error}")),
            );
            daemon
                .ok(
                    &request,
                    json!({"content": "", "live": true, "command": "echo hi", "sessionId": "s0"}),
                )
                .await;
            apply(&mut app, &mut rx).await;
            assert_eq!(app.panes.dev.content, "hi");
            assert!(!app.panes.dev.live);
            assert_eq!(app.data.repo_sessions[0].dev_running, Some(false));
        });
    }

    #[test]
    fn x_on_a_starting_session_cancels_it() {
        local(async {
            let (mut app, mut rx) = with_sessions(1);
            let mut daemon = Daemon::connect(&mut app);
            app.data.repo_sessions[0].status = SessionStatus::Starting;
            handle_browse(&mut app, &text("X"));
            let request = daemon.next().await;
            assert_eq!(request["type"], "cancel-start");
            daemon.fail(&request, "already started").await;
            apply(&mut app, &mut rx).await;
            assert_eq!(app.messages.error.as_deref(), Some("already started"));
        });
    }
}
