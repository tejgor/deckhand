//! Run list (r, pick-action): the worktree's Dev command first (when the session has a workspace), then the
//! repository's actions; untrusted repository ones are reviewed when chosen. ActionPickerPane renders it.
//!
//! Not in the TS: with named Dev processes (`devCommand` as a map), a row for each follows the Dev row, and the last
//! rows, the worktree's shells and `+ New shell`, come with the Dev row (all belong to the session's worktree).
//!
//! Not in the TS: every row follows one rule. Enter starts what is stopped and opens what runs (on its tab); x twice
//! ([`crate::press_again`]) stops the selected row, closes an extra shell or restarts the first one, killing whatever is
//! stuck in it. Restarting anything else is x x, then Enter. `+ New shell` opens another (at most [`MAX_SHELLS`]).

use std::rc::Rc;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use deckhand_client::live::LiveClient;
use deckhand_core::detail_texts::{ProjectAction, project_actions};
use deckhand_core::display::{DetailLine, HintPart, MenuItem, MenuPaneLine, RowStatus, THEME, error_message};
use deckhand_core::jsstr::js_trim;
use deckhand_core::key::Input;
use deckhand_protocol::data::{ActionRecord, ProjectInfo, RightPaneTab};

use crate::app::modes::browse::{dev_elsewhere, keep_newer_dev, run_dev, stopped_elsewhere, upsert_dev_running};
use crate::app::state::{ActionProject, App, AppAction, AppCallback, Mode, ReviewLabels, ReviewSkip};
use crate::app::{ProjectFn, ReviewOptions, hint_short, truthy};
use crate::keymap::{Binding, HelpRef, KeySpec, Keymap, NamedKey};
use crate::panes::dev::{dev_cell_size, dev_processes};
use crate::panes::menu::{MenuPaneDetails, MenuPaneProps, menu_pane_height, render_menu_dialog};
use crate::panes::terminal::{MAX_SHELLS, TerminalView, shell_label, shells};
use crate::primitives::{dialog_height, dialog_min_height, dialog_width};

/// pick-action keys.
pub static KEYMAP: Keymap = &[
    Binding {
        keys: &[
            KeySpec::Char("j"),
            KeySpec::Char("k"),
            KeySpec::Char("g"),
            KeySpec::Char("G"),
            KeySpec::Named(NamedKey::Up),
            KeySpec::Named(NamedKey::Down),
            KeySpec::Named(NamedKey::Home),
            KeySpec::Named(NamedKey::End),
        ],
        action: "move through the rows",
        help: HelpRef::Hidden("moving through a menu is what its on-screen hints say"),
    },
    Binding {
        keys: &[KeySpec::Char("x")],
        action: "twice: stop the selected row, close a shell or restart the first one",
        help: HelpRef::Topic("Dev & actions"),
    },
    Binding {
        keys: &[KeySpec::Named(NamedKey::Return)],
        action: "start what is stopped, open what runs, or open another shell",
        help: HelpRef::Topic("Dev & actions"),
    },
    Binding {
        keys: &[
            KeySpec::Named(NamedKey::Escape),
            KeySpec::Char("h"),
            KeySpec::Named(NamedKey::Left),
        ],
        action: "close the run list",
        help: HelpRef::Hidden("the pane's own hint says esc cancels"),
    },
    crate::keymap::SCROLL_DIALOG,
];

/// TS: r in browse: the run list of the selected session's repository (the running action watched first).
pub fn open_run_list(app: &mut App) {
    // The run list: the worktree's Dev command first, then every action of the session's repository (untrusted
    // repository ones are reviewed when chosen).
    let session = app.selected_session();
    let session_id = session.as_ref().map(|session| session.id.clone());
    let action_cwd = session
        .as_ref()
        .map_or_else(|| app.props.cwd.clone(), |session| session.cwd.clone());
    let watched_id = session_id.clone();
    let cwd = action_cwd.clone();
    let resume: AppAction<Option<ProjectInfo>> = Rc::new(move |app: &mut App, project: Option<ProjectInfo>| {
        let Some(project) = project else {
            return;
        };
        app.action_picker.action_project = Some(ActionProject {
            project,
            cwd: cwd.clone(),
            session_id: session_id.clone(),
        });
        app.action_picker.action_index = 0;
        app.action_picker.press_again.clear();
        app.set_mode(Mode::PickAction);
    });
    let options = ReviewOptions {
        required: true,
        gate: Some(Rc::new(|_: &ProjectInfo| false)),
        ..ReviewOptions::default()
    };
    // The picker says whether an action is still running in this worktree (x stops it) and which of Dev's named
    // processes run (Dev's own tab may not be watched), whichever tab is shown. Not in the TS: it opens once both
    // are in, so a row's state, its details and the hint do not change under it just after it opens.
    if let (Some(client), Some(id), true) = (
        app.client(),
        watched_id.as_ref().filter(|id| !id.is_empty()),
        truthy(app.selected_workspace().as_ref()),
    ) {
        let layout = app.layout();
        let action = client.watch_action(Some(id), i64::from(layout.preview_cols), i64::from(layout.preview_rows));
        let (cols, rows) = dev_size(app, dev_processes(&app.panes.dev).map_or(0, <[_]>::len));
        let dev = client.watch_dev(Some(id), i64::from(cols), i64::from(rows));
        // Busy (as reading the project is next): keys wait, the footer says it is working.
        app.set_busy(true);
        let task = app.dispatch().spawn(
            async move { (action.await, dev.await) },
            move |app: &mut App, (action, dev)| {
                if let Ok(record) = action
                    && record.session_id == app.selected_id_ref()
                {
                    app.panes.action = record;
                }
                if let Ok(record) = dev
                    && record.session_id == app.selected_id_ref()
                {
                    app.panes.dev = record;
                }
                app.set_busy(false);
                app.review_then(&action_cwd, Some(resume), options);
            },
        );
        drop(task);
        return;
    }
    app.review_then(&action_cwd, Some(resume), options);
}

/// The size of each Dev PTY with `count` named processes (the preview's size for a single command).
fn dev_size(app: &App, count: usize) -> (u16, u16) {
    let layout = app.layout();
    if count == 0 {
        return (layout.preview_cols, layout.preview_rows);
    }
    dev_cell_size(layout.pane_inner_width, layout.pane_inner_height, count)
}

/// Not in the TS: Enter on a Dev process row: starts it alone, or restarts it while it runs, then shows the Dev tab.
fn start_dev_process(app: &mut App, name: &str, running: bool, count: usize) {
    let (Some(client), Some(session)) = (app.client(), app.selected_session()) else {
        return;
    };
    app.set_busy(true);
    app.set_error(None);
    let (cols, rows) = dev_size(app, count);
    let elsewhere = dev_elsewhere(app, &session);
    let updates = app.panes.dev_updates;
    let reply = client.start_dev_process(&session.id, Some(name), running, i64::from(cols), i64::from(rows));
    let name = name.to_owned();
    let task = app.dispatch().spawn(reply, move |app: &mut App, result| {
        match result {
            Ok(next) => {
                let live = next.live;
                let live = keep_newer_dev(app, next, updates).unwrap_or(live);
                app.panes.dev_process = Some(name.clone());
                upsert_dev_running(app, &session, live);
                app.set_active_tab(RightPaneTab::Dev);
                if app.mode == Mode::PickAction {
                    app.set_mode(Mode::Browse);
                }
                let verb = if running { "Restarted" } else { "Started" };
                app.set_status(Some(format!(
                    "{verb} {name}{} · D stops Dev",
                    stopped_elsewhere(&elsewhere)
                )));
            }
            Err(error) => app.set_error(Some(error_message(&error))),
        }
        app.set_busy(false);
    });
    drop(task);
}

/// Not in the TS: x on a running Dev process row stops it (the other processes keep running).
fn stop_dev_process(app: &mut App, name: &str) {
    let (Some(client), Some(session)) = (app.client(), app.selected_session()) else {
        return;
    };
    app.set_busy(true);
    let reply = client.stop_dev_process(&session.id, Some(name));
    let name = name.to_owned();
    let task = app.dispatch().spawn(reply, move |app: &mut App, result| {
        match result {
            Ok(()) => app.set_status(Some(format!("Stopped {name}"))),
            Err(error) => app.set_error(Some(error_message(&error))),
        }
        app.set_busy(false);
    });
    drop(task);
}

/// Not in the TS: Enter on a shell's row: restarts that shell (stopping everything running in it), then shows it on
/// the Terminal tab.
fn restart_shell(app: &mut App, shell: &ShellRow) {
    let (Some(client), Some(session)) = (app.client(), app.selected_session()) else {
        return;
    };
    app.set_busy(true);
    app.set_error(None);
    let layout = app.layout();
    let reply = client.restart_terminal(
        &session.id,
        (shell.id != 0).then_some(shell.id),
        i64::from(layout.preview_cols),
        i64::from(layout.preview_rows),
    );
    let (id, label) = (shell.id, shell.label.to_lowercase());
    let task = app.dispatch().spawn(reply, move |app: &mut App, result| {
        match result {
            Ok(record) => show_shell(app, record, id, format!("Restarted the {label}")),
            Err(error) => app.set_error(Some(error_message(&error))),
        }
        app.set_busy(false);
    });
    drop(task);
}

/// Not in the TS: Enter on `+ New shell`: another shell in the worktree, shown on the Terminal tab.
fn open_shell(app: &mut App) {
    let (Some(client), Some(session)) = (app.client(), app.selected_session()) else {
        return;
    };
    app.set_busy(true);
    app.set_error(None);
    let layout = app.layout();
    let reply = client.new_terminal(
        &session.id,
        i64::from(layout.preview_cols),
        i64::from(layout.preview_rows),
    );
    let task = app.dispatch().spawn(reply, move |app: &mut App, result| {
        match result {
            Ok(record) => {
                // Ids only grow: the newest shell has the largest.
                let id = shells(&record).iter().map(|shell| shell.id).max().unwrap_or(0);
                let label = shell_label(&record, id);
                show_shell(app, record, id, format!("Opened {label} · → types into it, v switches"));
            }
            Err(error) => app.set_error(Some(error_message(&error))),
        }
        app.set_busy(false);
    });
    drop(task);
}

/// Not in the TS: x on an extra shell's row: closes it (stopping everything running in it).
fn close_shell(app: &mut App, shell: &ShellRow) {
    let (Some(client), Some(session)) = (app.client(), app.selected_session()) else {
        return;
    };
    app.set_busy(true);
    app.set_error(None);
    let reply = client.close_terminal(&session.id, shell.id);
    let label = shell.label.to_lowercase();
    let task = app.dispatch().spawn(reply, move |app: &mut App, result| {
        match result {
            Ok(record) => {
                if record.session_id == app.selected_id_ref() {
                    app.panes.terminal = record;
                }
                app.action_picker.action_index = app.action_picker.action_index.saturating_sub(1);
                app.set_status(Some(format!("Closed {label}")));
            }
            Err(error) => app.set_error(Some(error_message(&error))),
        }
        app.set_busy(false);
    });
    drop(task);
}

/// Shows shell `id` of `record` on the Terminal tab, back on the session list.
fn show_shell(app: &mut App, record: deckhand_protocol::data::TerminalRecord, id: i64, status: String) {
    if record.session_id == app.selected_id_ref() {
        app.panes.terminal = record;
    }
    app.set_active_tab(RightPaneTab::Terminal);
    app.panes.terminal_view = TerminalView::Shell(id);
    if app.mode == Mode::PickAction {
        app.set_mode(Mode::Browse);
    }
    app.set_status(Some(status));
}

/// TS: the `run` closure of a picked action: starts it on the daemon and shows it on the Terminal tab.
fn run_picked(app: &mut App, client: &Rc<LiveClient>, session_id: &str, name: &str, cols: i64, rows: i64) {
    app.set_busy(true);
    let reply = client.run_action(session_id, name, cols, rows);
    let task = app.dispatch().spawn(reply, |app: &mut App, result| {
        match result {
            Ok(record) => {
                // The action runs in its own process; the Terminal tab shows it until v switches back to the shell.
                app.panes.action = record;
                app.set_active_tab(RightPaneTab::Terminal);
                app.panes.terminal_view = TerminalView::Action;
                if app.mode == Mode::PickAction {
                    app.set_mode(Mode::Browse);
                }
            }
            Err(error) => app.set_error(Some(error_message(&error))),
        }
        app.set_busy(false);
    });
    drop(task);
}

/// TS: Enter on an action: run it, after reviewing it first when it comes from an untrusted repository config.
fn run_action_row(app: &mut App, picked: &ProjectAction, action_project: &ActionProject) {
    let client = app.client();
    let session_id = action_project.session_id.clone();
    let (Some(client), Some(session_id)) = (client, session_id.filter(|id| !id.is_empty())) else {
        app.set_error(Some("select a session to run project actions".to_owned()));
        return;
    };
    let layout = app.layout();
    let (cols, rows) = (i64::from(layout.preview_cols), i64::from(layout.preview_rows));
    let name = picked.name.clone();
    let run: AppCallback = {
        let name = name.clone();
        Rc::new(move |app: &mut App| run_picked(app, &client, &session_id, &name, cols, rows))
    };
    if !picked.needs_trust {
        run(app);
        return;
    }
    // An untrusted repository action is reviewed now, right before it would run; Esc returns to the list.
    let back: AppCallback = Rc::new(|app: &mut App| app.set_mode(Mode::PickAction));
    let resume: AppAction<Option<ProjectInfo>> = {
        let run = Rc::clone(&run);
        Rc::new(move |app: &mut App, _: Option<ProjectInfo>| run(app))
    };
    let skip = if picked.fallback.is_none() {
        ReviewSkip::Cancel
    } else {
        let run = Rc::clone(&run);
        ReviewSkip::Run(Rc::new(move |app: &mut App, _: ProjectInfo| run(app)))
    };
    let skip_label = if picked.fallback.is_none() {
        hint_short("s cancel this run", "s cancel")
    } else {
        hint_short(&format!("s run the global {name} instead"), "s run global")
    };
    let labels = ReviewLabels {
        enter: Some(hint_short("enter trust & run", "enter trust")),
        skip: Some(skip_label),
    };
    let purpose = format!("About to run the repository action {name}: {}", picked.command);
    let options = ReviewOptions {
        back: Some(back),
        skip,
        labels: Some(Rc::new(move |_: &ProjectInfo| labels.clone()) as ProjectFn<ReviewLabels>),
        purpose: Some(Rc::new(move |_: &ProjectInfo| purpose.clone()) as ProjectFn<String>),
        ..ReviewOptions::default()
    };
    app.review_then(&action_project.cwd, Some(resume), options);
}

/// TS: the pick-action branch of the useTerminalInput handler.
pub fn handle_pick_action(app: &mut App, input: &Input) {
    let key = &input.key;
    let text = input.input.as_str();
    let action_project = app.action_picker.action_project.clone();
    let actions = project_actions(action_project.as_ref().map(|project| &project.project));
    let review_session_id = action_project.as_ref().and_then(|project| project.session_id.clone());
    // Row 0 is the worktree's Dev command when the session has a workspace; its named processes and the actions
    // follow, then the worktree's shell.
    let processes = app.run_list_processes();
    let has_dev = app.run_list_has_dev();
    let shells = app.run_list_shells();
    let dev_row = usize::from(has_dev) + processes.len();
    let shell_rows = dev_row + actions.len();
    let can_open = has_dev && shells.len() < MAX_SHELLS;
    let rows = shell_rows + shells.len() + usize::from(can_open);
    let last = rows.saturating_sub(1);
    // The handlers below read the index as it was when the key arrived (React state).
    let index = app.action_picker.action_index;
    // Not in the TS: h and ← close it too, as they go back from every pane.
    if key.escape || key.left_arrow || text == "h" {
        app.set_mode(Mode::Browse);
        return;
    }
    if key.up_arrow || text == "k" {
        app.action_picker.action_index = app.action_picker.action_index.saturating_sub(1);
    }
    if key.down_arrow || text == "j" {
        app.action_picker.action_index = last.min(app.action_picker.action_index + 1);
    }
    if text == "g" || key.home {
        app.action_picker.action_index = 0;
    }
    if text == "G" || key.end {
        app.action_picker.action_index = last;
    }
    // Not in the TS: one rule for every row. Enter starts what is stopped and opens what runs; x twice stops or closes
    // the selected row (on the first shell, restarts it); restarting is x x then Enter.
    let armed = app.action_picker.press_again.take();
    if text != "x" {
        if let Some(shell) = index.checked_sub(shell_rows).and_then(|row| shells.get(row)) {
            if key.return_ {
                show_running_shell(app, shell.id);
            }
        } else if can_open && index == shell_rows + shells.len() {
            if key.return_ {
                open_shell(app);
            }
        } else if let Some(process) = index.checked_sub(1).and_then(|row| processes.get(row)) {
            if key.return_ && process.running {
                app.panes.dev_process = Some(process.name.clone());
                show_tab_from_run_list(app, RightPaneTab::Dev);
            } else if key.return_ {
                start_dev_process(app, &process.name, false, processes.len());
            }
        } else if dev_row >= 1 && index == 0 {
            if key.return_ && run_list_dev_runs(app) {
                show_tab_from_run_list(app, RightPaneTab::Dev);
            } else if key.return_ {
                let back: AppCallback = Rc::new(|app: &mut App| app.set_mode(Mode::PickAction));
                run_dev(app, Some(back));
            }
        } else if key.return_
            && let Some(picked) = index.checked_sub(dev_row).and_then(|row| actions.get(row))
        {
            if running_action(app, review_session_id.as_deref()) == Some(picked.name.as_str()) {
                app.panes.terminal_view = TerminalView::Action;
                show_tab_from_run_list(app, RightPaneTab::Terminal);
            } else if let Some(action_project) = action_project.as_ref() {
                run_action_row(app, picked, action_project);
            }
        }
        return;
    }
    // x: what the selected row would stop, and its menu key (what a second x must land on).
    let target = if let Some(shell) = index.checked_sub(shell_rows).and_then(|row| shells.get(row)) {
        let name = shell_noun(&shell.label);
        Some((
            format!("__shell:{}", shell.id),
            if shell.id == 0 {
                format!("restarts {name}")
            } else {
                format!("closes {name}")
            },
        ))
    } else if can_open && index == shell_rows + shells.len() {
        None
    } else if let Some(process) = index.checked_sub(1).and_then(|row| processes.get(row)) {
        process
            .running
            .then(|| (format!("__dev:{}", process.name), format!("stops {}", process.name)))
    } else if dev_row >= 1 && index == 0 {
        run_list_dev_runs(app).then(|| ("__dev".to_owned(), "stops Dev".to_owned()))
    } else {
        index
            .checked_sub(dev_row)
            .and_then(|row| actions.get(row))
            .filter(|picked| running_action(app, review_session_id.as_deref()) == Some(picked.name.as_str()))
            .map(|picked| (picked.name.clone(), format!("stops {}", picked.name)))
    };
    let Some((row_key, what)) = target else {
        app.set_status(Some("Nothing runs on this row".to_owned()));
        return;
    };
    if armed.as_deref() != Some(row_key.as_str()) {
        app.action_picker.press_again.arm(row_key);
        app.set_status(Some(format!("x again {what}")));
        return;
    }
    if let Some(shell) = index.checked_sub(shell_rows).and_then(|row| shells.get(row)) {
        if shell.id == 0 {
            restart_shell(app, shell);
        } else {
            close_shell(app, shell);
        }
    } else if let Some(process) = index.checked_sub(1).and_then(|row| processes.get(row)) {
        stop_dev_process(app, &process.name);
    } else if dev_row >= 1 && index == 0 {
        let back: AppCallback = Rc::new(|app: &mut App| app.set_mode(Mode::PickAction));
        run_dev(app, Some(back));
    } else if let Some(session_id) = review_session_id.filter(|id| !id.is_empty()) {
        stop_running_action(app, &session_id);
    }
}

/// A shell's row label in a sentence: `the shell` when it is the only one, else `shell 2`.
fn shell_noun(label: &str) -> String {
    let label = label.to_lowercase();
    if label == "shell" {
        "the shell".to_owned()
    } else {
        label
    }
}

/// Whether the selected session's worktree runs Dev (its record, or the Dev pane watching it live).
fn run_list_dev_runs(app: &App) -> bool {
    let selected = app.selected_session();
    selected
        .as_ref()
        .is_some_and(|session| session.dev_running == Some(true))
        || (app.panes.dev.session_id.as_deref() == selected.as_ref().map(|session| session.id.as_str())
            && app.panes.dev.live)
}

/// The action running in the worktree of `session_id` (one at a time), by name.
fn running_action<'a>(app: &'a App, session_id: Option<&str>) -> Option<&'a str> {
    let action = &app.panes.action;
    (action.live && session_id.is_some_and(|id| !id.is_empty()) && action.session_id.as_deref() == session_id)
        .then(|| action.name.as_deref().unwrap_or("action"))
}

/// Enter on a running row: shows it on its tab, back on the session list.
fn show_tab_from_run_list(app: &mut App, tab: RightPaneTab) {
    app.set_active_tab(tab);
    app.set_mode(Mode::Browse);
}

/// Enter on a shell's row: shows that shell on the Terminal tab.
fn show_running_shell(app: &mut App, id: i64) {
    app.panes.terminal_view = TerminalView::Shell(id);
    show_tab_from_run_list(app, RightPaneTab::Terminal);
}

/// x twice on the running action's row: stops it.
fn stop_running_action(app: &mut App, session_id: &str) {
    let Some(client) = app.client() else { return };
    app.set_busy(true);
    let name = app.panes.action.name.clone().unwrap_or_else(|| "the action".to_owned());
    let reply = client.stop_action(session_id);
    let task = app.dispatch().spawn(reply, move |app: &mut App, result| {
        match result {
            Ok(()) => app.set_status(Some(format!("Stopped {name}"))),
            Err(error) => app.set_error(Some(error_message(&error))),
        }
        app.set_busy(false);
    });
    drop(task);
}

/// TS: ActionPickerPane `dev`: the Dev row and whether it runs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DevRow {
    pub running: bool,
}

/// Not in the TS: the row of one of the worktree's shells (`Shell`, or `Shell 2` with several).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ShellRow {
    pub id: i64,
    pub label: String,
}

/// Not in the TS: the row of one named Dev process.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DevProcessRow {
    pub name: String,
    pub command: String,
    pub running: bool,
}

/// TS: app.tsx ActionPickerPane props (width and height are the area's).
#[derive(Clone, Copy, Debug)]
pub struct ActionPickerPaneProps<'a> {
    pub project: Option<&'a ProjectInfo>,
    /// The worktree's last action, when it belongs to the picker's session.
    pub running: Option<&'a ActionRecord>,
    pub dev: Option<DevRow>,
    /// Not in the TS: Dev's named processes, listed after the Dev row.
    pub processes: &'a [DevProcessRow],
    /// Not in the TS: the worktree's shells, listed after the actions (with the Dev row), then `+ New shell`.
    pub shells: &'a [ShellRow],
    pub selected_index: usize,
}

/// A details line with a color.
fn colored(text: String, color: &'static str) -> DetailLine {
    DetailLine {
        text,
        color: Some(color),
        ..DetailLine::default()
    }
}

/// A details line cut instead of wrapped.
fn command_line(text: String) -> DetailLine {
    DetailLine {
        nowrap: Some(true),
        ..colored(text, THEME.active)
    }
}

/// The run list's content for `props`' selection (a MenuPane's, owned).
struct RunList {
    subtitle: [MenuPaneLine; 1],
    items: Vec<MenuItem>,
    details: Option<MenuPaneDetails>,
    hint: Vec<HintPart>,
}

impl RunList {
    fn pane(&self, selected: usize) -> MenuPaneProps<'_> {
        MenuPaneProps {
            title: "Run",
            subtitle: &self.subtitle,
            items: &self.items,
            selected,
            details: self.details.as_ref(),
            empty: Some("No actions configured (C → Settings › Actions adds one)"),
            hint: &self.hint,
            label_width: None,
        }
    }
}

/// TS: app.tsx ActionPickerPane (a MenuPane), as a dialog as tall as its tallest row's details need, so it keeps
/// its size as the selection moves.
pub fn render_action_picker_pane(props: &ActionPickerPaneProps<'_>, buf: &mut Buffer, area: Rect) {
    let width = usize::from(dialog_width(area));
    let count = run_list(props).items.len().max(1);
    let height = (0..count)
        .map(|index| {
            let other = ActionPickerPaneProps {
                selected_index: index,
                ..*props
            };
            let list = run_list(&other);
            // The pane's height counts its border; the dialog's rows do not.
            dialog_height(area, menu_pane_height(&list.pane(index), width).saturating_sub(2), 0)
        })
        .max()
        .unwrap_or_else(|| dialog_min_height(area));
    let list = run_list(props);
    render_menu_dialog(&list.pane(props.selected_index), buf, area, height);
}

/// The run list's content (TS: ActionPickerPane's body).
fn run_list(props: &ActionPickerPaneProps<'_>) -> RunList {
    let project = props.project;
    let actions = project_actions(project);
    let processes = if props.dev.is_some() { props.processes } else { &[] };
    let dev_command = if processes.is_empty() {
        project
            .and_then(|project| project.effective.dev_command_text())
            .map(js_trim)
            .filter(|command| !command.is_empty())
            .unwrap_or("dev")
            .to_owned()
    } else {
        processes
            .iter()
            .map(|process| process.name.as_str())
            .collect::<Vec<_>>()
            .join(" + ")
    };
    let dev_offset = usize::from(props.dev.is_some()) + processes.len();
    let on_dev = props.dev.is_some() && props.selected_index == 0;
    let on_process = props
        .selected_index
        .checked_sub(1)
        .and_then(|row| processes.get(row))
        .filter(|_| props.dev.is_some());
    let selected = props
        .selected_index
        .checked_sub(dev_offset)
        .and_then(|row| actions.get(row));
    // With the Dev row the worktree has at least its first shell.
    let first = [ShellRow {
        id: 0,
        label: "Shell".to_owned(),
    }];
    let shells = match props.dev {
        Some(_) if props.shells.is_empty() => &first[..],
        Some(_) => props.shells,
        None => &[],
    };
    let shell_offset = dev_offset + actions.len();
    let on_shell = props
        .selected_index
        .checked_sub(shell_offset)
        .and_then(|row| shells.get(row));
    let can_open = props.dev.is_some() && shells.len() < MAX_SHELLS;
    let on_new_shell = can_open && props.selected_index == shell_offset + shells.len();
    let untrusted_dev = props.dev.is_some()
        && project.is_some_and(|project| {
            project.needs_review
                && project
                    .project
                    .config
                    .dev_command
                    .as_ref()
                    .is_some_and(|dev| dev.text() != Some(""))
        });
    let untrusted = actions.iter().any(|action| action.needs_trust) || untrusted_dev;
    // TS `running?.live ? running.name ?? 'action' : undefined`, used for its truthiness and text.
    let running_name = props
        .running
        .filter(|running| running.live)
        .map(|running| running.name.clone().unwrap_or_else(|| "action".to_owned()))
        .filter(|name| !name.is_empty());

    let subtitle = [if untrusted {
        MenuPaneLine {
            text: "\"needs trust\": from deckhand.json, reviewed first".to_owned(),
            color: Some(THEME.warn),
        }
    } else {
        MenuPaneLine {
            text: "Dev, then your actions (global defaults and repository)".to_owned(),
            color: None,
        }
    }];
    let mut items = Vec::new();
    if let Some(dev) = props.dev {
        items.push(MenuItem {
            key: "__dev".to_owned(),
            label: "Dev".to_owned(),
            description: Some(dev_command.clone()),
            status: Some(if dev.running {
                RowStatus {
                    text: "● running".to_owned(),
                    color: Some(THEME.success),
                }
            } else {
                RowStatus {
                    text: "○ stopped".to_owned(),
                    color: Some(THEME.muted),
                }
            }),
            ..MenuItem::default()
        });
    }
    items.extend(processes.iter().map(|process| MenuItem {
        key: format!("__dev:{}", process.name),
        label: format!("  {}", process.name),
        description: Some(process.command.clone()),
        status: Some(if process.running {
            RowStatus {
                text: "● running".to_owned(),
                color: Some(THEME.success),
            }
        } else {
            RowStatus {
                text: "○ stopped".to_owned(),
                color: Some(THEME.muted),
            }
        }),
        ..MenuItem::default()
    }));
    items.extend(actions.iter().map(|action| MenuItem {
        key: action.name.clone(),
        label: action.name.clone(),
        description: Some(action.command.clone()),
        status: if running_name.as_deref() == Some(action.name.as_str()) {
            Some(RowStatus {
                text: "● running".to_owned(),
                color: Some(THEME.success),
            })
        } else {
            action.needs_trust.then(|| RowStatus {
                text: "· needs trust".to_owned(),
                color: Some(THEME.warn),
            })
        },
        ..MenuItem::default()
    }));
    items.extend(shells.iter().map(|shell| {
        MenuItem {
            key: format!("__shell:{}", shell.id),
            label: shell.label.clone(),
            description: Some(
                if shells.len() < 2 {
                    "the worktree's shell · x restarts it"
                } else if shell.id == 0 {
                    "x restarts it"
                } else {
                    "x closes it"
                }
                .to_owned(),
            ),
            ..MenuItem::default()
        }
    }));
    if can_open {
        items.push(MenuItem {
            key: "__new_shell".to_owned(),
            label: "+ New shell".to_owned(),
            description: Some("another shell in this worktree".to_owned()),
            ..MenuItem::default()
        });
    }

    let details = if let Some(process) = on_process {
        let mut lines = vec![
            command_line(process.command.clone()),
            DetailLine {
                text: if process.running {
                    "Running: Enter shows it, x twice stops it; the other Dev processes keep running.".to_owned()
                } else {
                    "Enter starts it alone; Enter on Dev (or D) starts every process.".to_owned()
                },
                ..DetailLine::default()
            },
        ];
        if let Some(name) = &running_name {
            lines.push(colored(
                format!("{name} is still running in this worktree: x twice on its row stops it."),
                THEME.warn,
            ));
        }
        Some(MenuPaneDetails {
            title: format!("Dev · {}", process.name),
            lines,
        })
    } else if let Some(shell) = on_shell {
        let mut lines = vec![DetailLine {
            text: if shell.id == 0 {
                "Enter shows it. x twice stops the shell and everything running in it, then starts a fresh one. Anything that ignores the hangup is killed 2 s later."
            } else {
                "Enter shows it. x twice closes it, stopping everything running in it."
            }
            .to_owned(),
            ..DetailLine::default()
        }];
        lines.push(colored(
            "Every session in this worktree shares its shells; Dev and actions keep running.".to_owned(),
            THEME.muted,
        ));
        if let Some(name) = &running_name {
            lines.push(colored(
                format!("{name} is still running in this worktree: x twice on its row stops it."),
                THEME.warn,
            ));
        }
        Some(MenuPaneDetails {
            title: shell.label.clone(),
            lines,
        })
    } else if on_new_shell {
        Some(MenuPaneDetails {
            title: "New shell".to_owned(),
            lines: vec![
                DetailLine {
                    text: format!(
                        "Enter opens another $SHELL in this worktree, beside the others (at most {MAX_SHELLS}). The Terminal tab shows one at a time: v switches, → types into the one shown."
                    ),
                    ..DetailLine::default()
                },
                colored(
                    "Every session in this worktree sees the same shells. One that exits is gone.".to_owned(),
                    THEME.muted,
                ),
            ],
        })
    } else if on_dev {
        let running = props.dev.is_some_and(|dev| dev.running);
        let mut lines = vec![
            command_line(dev_command.clone()),
            DetailLine {
                text: if running {
                    "Running in this worktree: Enter shows it, x twice stops it.".to_owned()
                } else {
                    "Enter starts it in this worktree; its output is on the Dev tab, shared by every session there."
                        .to_owned()
                },
                ..DetailLine::default()
            },
        ];
        if let Some(name) = &running_name {
            lines.push(colored(
                format!("{name} is still running in this worktree: x twice on its row stops it."),
                THEME.warn,
            ));
        }
        Some(MenuPaneDetails {
            title: "Dev".to_owned(),
            lines,
        })
    } else {
        selected.map(|selected| {
            let mut lines = vec![command_line(selected.command.clone())];
            if selected.needs_trust {
                let fallback = match selected.fallback.as_ref() {
                    Some(fallback) if !fallback.is_empty() => {
                        format!(" (s runs the global {} instead: {fallback})", selected.name)
                    }
                    _ => String::new(),
                };
                lines.push(colored(
                    format!(
                        "From this repo's deckhand.json, not trusted yet: Enter shows it for review before anything runs{fallback}."
                    ),
                    THEME.warn,
                ));
            }
            lines.push(match &running_name {
                Some(name) if *name == selected.name => DetailLine {
                    text: "Running on the Terminal tab: Enter shows it, x twice stops it.".to_owned(),
                    ..DetailLine::default()
                },
                Some(name) => colored(
                    format!("{name} is still running in this worktree: x twice on its row stops it, then run another."),
                    THEME.warn,
                ),
                None => DetailLine {
                    text: "Runs on the Terminal tab beside your shell (v switches), one action at a time per worktree; Dev keeps running.".to_owned(),
                    ..DetailLine::default()
                },
            });
            MenuPaneDetails {
                title: "Selected command".to_owned(),
                lines,
            }
        })
    };

    // Enter starts what is stopped and opens what runs; x (twice) stops the selected row only.
    let dev_running = props.dev.is_some_and(|dev| dev.running);
    let action_running = selected.is_some_and(|selected| running_name.as_deref() == Some(selected.name.as_str()));
    let (enter, x) = if let Some(process) = on_process {
        if process.running {
            (
                format!("enter show {}", process.name),
                Some(format!("x stop {}", process.name)),
            )
        } else {
            (format!("enter start {}", process.name), None)
        }
    } else if let Some(shell) = on_shell {
        let name = shell_noun(&shell.label);
        (
            format!("enter show {name}"),
            Some(if shell.id == 0 {
                format!("x restart {name}")
            } else {
                format!("x close {name}")
            }),
        )
    } else if on_new_shell {
        ("enter open a shell".to_owned(), None)
    } else if on_dev {
        if dev_running {
            ("enter show Dev".to_owned(), Some("x stop Dev".to_owned()))
        } else {
            ("enter start Dev".to_owned(), None)
        }
    } else if action_running {
        let name = running_name.clone().unwrap_or_default();
        (format!("enter show {name}"), Some(format!("x stop {name}")))
    } else {
        ("enter run".to_owned(), None)
    };
    let mut hint: Vec<HintPart> = vec![HintPart::from(enter.as_str())];
    if let Some(x) = x {
        hint.push(HintPart::from(x.as_str()));
    }
    hint.push(HintPart::from("esc cancel"));

    RunList {
        subtitle,
        items,
        details,
        hint,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;
    use crate::app::event_loop::Update;
    use crate::app::modes::workspace_info::fake_daemon::{connect, local, project_info, settle};
    use crate::app::state::AppProps;
    use crate::app::test_support::{app, session};
    use deckhand_core::key::Key;
    use deckhand_protocol::data::SessionStatus;
    use tokio::sync::mpsc::UnboundedReceiver;

    fn project(config: Value, effective: Value, trusted: bool, needs_review: bool) -> ProjectInfo {
        serde_json::from_value(project_info(config, effective, trusted, needs_review))
            .unwrap_or_else(|error| panic!("{error}"))
    }

    /// The golden's repository: an untrusted deckhand.json with a devCommand and one action.
    fn untrusted() -> ProjectInfo {
        project(
            json!({"devCommand": "echo dev-ready", "actions": {"test": "echo tests-ok"}}),
            json!({}),
            false,
            true,
        )
    }

    /// Trusted: its actions apply (`build`, `lint`).
    fn trusted() -> ProjectInfo {
        let actions = json!({"build": "make build", "lint": "make lint"});
        project(
            json!({"actions": actions}),
            json!({"actions": actions, "devCommand": " make dev "}),
            true,
            false,
        )
    }

    fn rows_of(buf: &Buffer) -> Vec<String> {
        (0..buf.area.height)
            .map(|y| {
                let mut line = String::new();
                let mut x = 0;
                while x < buf.area.width {
                    let symbol = buf[(x, y)].symbol();
                    line.push_str(symbol);
                    x += u16::try_from(crate::primitives::string_width(symbol).max(1)).unwrap_or(1);
                }
                line.trim_end().to_owned()
            })
            .collect()
    }

    fn render(props: &ActionPickerPaneProps<'_>, width: u16, height: u16) -> Vec<String> {
        let area = Rect::new(0, 0, width, height);
        let mut buf = Buffer::empty(area);
        render_action_picker_pane(props, &mut buf, area);
        rows_of(&buf)
    }

    /// The dialog's text with its borders taken off and its rows joined by spaces, so a sentence reads whole
    /// wherever the dialog's width wrapped it.
    fn shown(rows: &[String]) -> String {
        rows.iter()
            .map(|row| row.trim_matches(|c| c == ' ' || c == '│'))
            .filter(|row| !row.is_empty())
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn picker_app() -> (App, UnboundedReceiver<Update<App>>) {
        let (mut app, rx) = app(AppProps::default());
        app.data.repo_sessions = vec![session("a", "Alpha", SessionStatus::Running)];
        app.set_selected_id(Some("a".to_owned()));
        (app, rx)
    }

    fn open_with(app: &mut App, project: ProjectInfo, session_id: Option<&str>) {
        app.action_picker.action_project = Some(ActionProject {
            project,
            cwd: "/repo".to_owned(),
            session_id: session_id.map(str::to_owned),
        });
        app.action_picker.action_index = 0;
        app.set_mode(Mode::PickAction);
    }

    fn pressed(set: impl FnOnce(&mut Key)) -> Input {
        let mut key = Key::NONE;
        set(&mut key);
        Input::key("", key)
    }

    fn enter() -> Input {
        pressed(|key| key.return_ = true)
    }

    fn action_record(live: bool) -> Value {
        json!({
            "content": "", "live": live, "name": "build", "command": "make build", "sessionId": "a",
            "workspace": "/repo"
        })
    }

    #[test]
    fn the_run_list_renders_like_the_snapshot() {
        let project = untrusted();
        let props = ActionPickerPaneProps {
            project: Some(&project),
            running: None,
            dev: Some(DevRow { running: false }),
            processes: &[],
            shells: &[],
            selected_index: 0,
        };
        // The right pane of the 130x36 app: 98 columns, 31 rows.
        let got = render(&props, 98, 31);
        deckhand_testkit::assert_snapshot("fixtures/ui/panes/43-run-list.txt", &got.join("\n"));
    }

    #[test]
    fn the_pane_lists_dev_then_the_actions_with_the_selected_ones_details() {
        let project = trusted();
        let props = ActionPickerPaneProps {
            project: Some(&project),
            running: None,
            dev: Some(DevRow { running: true }),
            processes: &[],
            shells: &[],
            selected_index: 0,
        };
        let text = shown(&render(&props, 100, 24));
        assert!(text.contains("Dev, then your actions (global defaults and repository)"));
        assert!(text.contains("Dev          make dev") && text.contains("● running"));
        assert!(text.contains("build        make build"));
        assert!(text.contains("Running in this worktree: Enter shows it, x twice stops it."));
        assert!(text.contains("enter show Dev • x stop Dev • esc cancel"));
        let on_build = ActionPickerPaneProps {
            selected_index: 1,
            ..props
        };
        let text = shown(&render(&on_build, 100, 24));
        assert!(text.contains("Selected command"));
        assert!(text.contains("Runs on the Terminal tab beside your shell (v switches)"));
        assert!(text.contains("enter run • esc cancel"));
        // Without a workspace there is no Dev row and the first action is row 0.
        let no_dev = ActionPickerPaneProps {
            dev: None,
            selected_index: 1,
            ..props
        };
        let text = shown(&render(&no_dev, 100, 24));
        assert!(!text.contains("Dev    make dev"));
        assert!(text.contains("❯ lint   make lint"));
    }

    #[test]
    fn a_running_action_is_named_and_only_its_row_offers_x() {
        let project = trusted();
        let record: ActionRecord = serde_json::from_value(action_record(true)).unwrap_or_default();
        let on_dev = ActionPickerPaneProps {
            project: Some(&project),
            running: Some(&record),
            dev: Some(DevRow { running: false }),
            processes: &[],
            shells: &[],
            selected_index: 0,
        };
        let text = shown(&render(&on_dev, 100, 24));
        assert!(text.contains("build is still running in this worktree: x twice on its row stops it."));
        assert!(text.contains("enter start Dev • esc cancel"));
        // Its own row says it runs; Enter shows it and x stops it.
        let on_build = ActionPickerPaneProps {
            selected_index: 1,
            ..on_dev
        };
        let text = shown(&render(&on_build, 100, 24));
        assert!(
            text.contains("build        make build") && text.contains("● running"),
            "{text}"
        );
        assert!(text.contains("Running on the Terminal tab: Enter shows it, x twice stops it."));
        assert!(text.contains("enter show build • x stop build • esc cancel"));
        let on_action = ActionPickerPaneProps {
            selected_index: 2,
            ..on_dev
        };
        let text = shown(&render(&on_action, 100, 24));
        assert!(
            text.contains("build is still running in this worktree: x twice on its row stops it, then run another.")
        );
        assert!(text.contains("enter run • esc cancel"));
        // Dev running beside a running action: its row offers its own.
        let both = ActionPickerPaneProps {
            dev: Some(DevRow { running: true }),
            ..on_dev
        };
        assert!(shown(&render(&both, 100, 24)).contains("enter show Dev • x stop Dev • esc cancel"));
        // A finished action is not running.
        let done: ActionRecord = serde_json::from_value(action_record(false)).unwrap_or_default();
        let finished = ActionPickerPaneProps {
            running: Some(&done),
            ..on_dev
        };
        assert!(!shown(&render(&finished, 100, 24)).contains("still running"));
        // A nameless running action is "action".
        let mut nameless = record.clone();
        nameless.name = None;
        let unnamed = ActionPickerPaneProps {
            running: Some(&nameless),
            ..on_dev
        };
        assert!(shown(&render(&unnamed, 100, 24)).contains("action is still running"));
    }

    #[test]
    fn untrusted_actions_say_what_enter_does_and_offer_the_global_one() {
        let project = project(
            json!({"actions": {"build": "make evil", "lint": "make lint"}}),
            json!({"actions": {"build": "make build"}}),
            false,
            true,
        );
        let props = ActionPickerPaneProps {
            project: Some(&project),
            running: None,
            dev: None,
            processes: &[],
            shells: &[],
            selected_index: 0,
        };
        let text = shown(&render(&props, 110, 24));
        assert!(text.contains("\"needs trust\": from deckhand.json, reviewed first"));
        assert!(text.contains("· needs trust"));
        assert!(text.contains(
            "From this repo's deckhand.json, not trusted yet: Enter shows it for review before anything runs (s"
        ));
        assert!(text.contains("runs the global build instead: make build)."));
        let lint = ActionPickerPaneProps {
            selected_index: 1,
            ..props
        };
        let text = shown(&render(&lint, 110, 24));
        assert!(text.contains("Enter shows it for review before anything runs."));
    }

    #[test]
    fn an_empty_list_says_how_to_add_an_action() {
        let project = project(json!({}), json!({}), true, false);
        let props = ActionPickerPaneProps {
            project: Some(&project),
            running: None,
            dev: None,
            processes: &[],
            shells: &[],
            selected_index: 0,
        };
        assert!(shown(&render(&props, 100, 12)).contains("No actions configured (C → Settings › Actions adds one)"));
        let none = ActionPickerPaneProps { project: None, ..props };
        assert!(shown(&render(&none, 100, 12)).contains("No actions configured"));
    }

    #[test]
    fn the_cursor_moves_over_the_dev_row_and_the_actions() {
        let (mut app, _rx) = picker_app();
        open_with(&mut app, trusted(), Some("a"));
        // Dev, build, lint, Shell, + New shell.
        for (input, want) in [
            ("j", 1),
            ("j", 2),
            ("j", 3),
            ("j", 4),
            ("j", 4),
            ("k", 3),
            ("g", 0),
            ("k", 0),
            ("G", 4),
        ] {
            handle_pick_action(&mut app, &Input::text(input));
            assert_eq!(app.action_picker.action_index, want, "{input}");
        }
        handle_pick_action(&mut app, &pressed(|key| key.up_arrow = true));
        assert_eq!(app.action_picker.action_index, 3);
        handle_pick_action(&mut app, &pressed(|key| key.down_arrow = true));
        assert_eq!(app.action_picker.action_index, 4);
        handle_pick_action(&mut app, &pressed(|key| key.home = true));
        assert_eq!(app.action_picker.action_index, 0);
        handle_pick_action(&mut app, &pressed(|key| key.end = true));
        assert_eq!(app.action_picker.action_index, 4);
        handle_pick_action(&mut app, &pressed(|key| key.escape = true));
        assert_eq!(app.mode, Mode::Browse);
        // A session without a workspace has no Dev or Shell row: two rows.
        open_with(&mut app, trusted(), Some("zzz"));
        handle_pick_action(&mut app, &Input::text("G"));
        assert_eq!(app.action_picker.action_index, 1);
        // No rows at all stays on 0.
        open_with(&mut app, project(json!({}), json!({}), true, false), None);
        handle_pick_action(&mut app, &Input::text("j"));
        handle_pick_action(&mut app, &Input::text("G"));
        assert_eq!(app.action_picker.action_index, 0);
    }

    fn watch_and_run_daemon(message: &Value) -> Result<Value, String> {
        match message["type"].as_str() {
            Some("watch-action") | Some("run-action") => Ok(action_record(true)),
            Some("stop-action") => Ok(Value::Null),
            Some("project-info") => Ok(project_info(
                json!({"actions": {"build": "make build"}}),
                json!({"actions": {"build": "make build"}}),
                true,
                false,
            )),
            other => Err(format!("unexpected {other:?}")),
        }
    }

    #[test]
    fn r_watches_the_action_and_opens_the_list_of_the_sessions_repository() {
        local(async {
            let (mut app, mut rx) = picker_app();
            let seen = connect(&mut app, watch_and_run_daemon);
            app.action_picker.action_index = 5;
            open_run_list(&mut app);
            // reviewThen is busy until the project is read.
            assert!(app.messages.busy);
            settle(&mut app, &mut rx).await;
            assert_eq!(app.mode, Mode::PickAction);
            assert!(!app.messages.busy);
            let opened = app
                .action_picker
                .action_project
                .clone()
                .unwrap_or_else(|| panic!("no project"));
            assert_eq!(
                (
                    opened.cwd.as_str(),
                    opened.session_id.as_deref(),
                    app.action_picker.action_index
                ),
                ("/repo", Some("a"), 0)
            );
            // The watched action is kept (it belongs to the selected session).
            assert!(app.panes.action.live && app.panes.action.session_id.as_deref() == Some("a"));
            let seen = seen.borrow();
            let layout = app.layout();
            let watch = seen.iter().find(|message| message["type"] == "watch-action");
            let watch = watch.unwrap_or_else(|| panic!("no watch-action in {seen:?}"));
            assert_eq!(watch["sessionId"], "a");
            assert_eq!(watch["cols"], i64::from(layout.preview_cols));
            assert_eq!(watch["rows"], i64::from(layout.preview_rows));
            assert!(
                seen.iter()
                    .any(|message| message["type"] == "project-info" && message["cwd"] == "/repo")
            );
            // The running states are asked for first: the list opens with them, so its rows do not change after.
            let at = |kind: &str| seen.iter().position(|message| message["type"] == kind);
            assert!(at("watch-action").is_some() && at("watch-dev").is_some());
            assert!(at("watch-action") < at("project-info") && at("watch-dev") < at("project-info"));
        });
    }

    #[test]
    fn r_without_a_session_lists_the_launch_directorys_actions_and_without_a_client_does_nothing() {
        local(async {
            let (mut app, mut rx) = app(AppProps::default());
            open_run_list(&mut app);
            assert_eq!((app.mode, app.messages.busy), (Mode::Browse, false));
            let seen = connect(&mut app, watch_and_run_daemon);
            app.props.cwd = "/launch".to_owned();
            open_run_list(&mut app);
            settle(&mut app, &mut rx).await;
            assert_eq!(app.mode, Mode::PickAction);
            let opened = app
                .action_picker
                .action_project
                .clone()
                .unwrap_or_else(|| panic!("no project"));
            assert_eq!((opened.cwd.as_str(), opened.session_id), ("/launch", None));
            assert!(seen.borrow().iter().all(|message| message["type"] != "watch-action"));
            // No Dev row without a session: the list has the one action.
            assert!(!app.run_list_has_dev());
        });
    }

    #[test]
    fn r_reports_an_unreadable_config_and_stays_on_the_list() {
        local(async {
            let (mut app, mut rx) = picker_app();
            let _seen = connect(&mut app, |message| match message["type"].as_str() {
                Some("project-info") => Err("deckhand.json is not valid JSON".to_owned()),
                _ => Ok(action_record(false)),
            });
            open_run_list(&mut app);
            settle(&mut app, &mut rx).await;
            assert_eq!(app.mode, Mode::Browse);
            assert_eq!(app.messages.error.as_deref(), Some("deckhand.json is not valid JSON"));
            assert!(app.action_picker.action_project.is_none() && !app.messages.busy);
        });
    }

    #[test]
    fn enter_on_a_trusted_action_runs_it_on_the_terminal_tab() {
        local(async {
            let (mut app, mut rx) = picker_app();
            let seen = connect(&mut app, watch_and_run_daemon);
            open_with(&mut app, trusted(), Some("a"));
            // Row 0 is Dev, row 1 is build.
            handle_pick_action(&mut app, &Input::text("j"));
            handle_pick_action(&mut app, &enter());
            assert!(app.messages.busy);
            settle(&mut app, &mut rx).await;
            assert!(!app.messages.busy);
            assert_eq!(app.mode, Mode::Browse);
            assert_eq!(app.browse.active_tab, RightPaneTab::Terminal);
            assert_eq!(app.panes.terminal_view, TerminalView::Action);
            assert!(app.panes.action.live);
            let seen = seen.borrow();
            let layout = app.layout();
            assert_eq!(seen.len(), 1);
            assert_eq!(
                (
                    seen[0]["type"].as_str(),
                    seen[0]["sessionId"].as_str(),
                    seen[0]["action"].as_str()
                ),
                (Some("run-action"), Some("a"), Some("build"))
            );
            assert_eq!(seen[0]["cols"], i64::from(layout.preview_cols));
            assert_eq!(seen[0]["rows"], i64::from(layout.preview_rows));
        });
    }

    /// Trusted, with named Dev processes (`api`, `web`) and one action.
    fn with_processes() -> ProjectInfo {
        let processes = json!({"api": "cd backend && python3 main.py", "web": "cd frontend && npm run dev"});
        project(
            json!({"devCommand": processes, "actions": {"build": "make build"}}),
            json!({"devCommand": processes, "actions": {"build": "make build"}}),
            true,
            false,
        )
    }

    #[test]
    fn dev_processes_get_rows_that_start_show_and_stop_each_one() {
        local(async {
            let (mut app, mut rx) = picker_app();
            let seen = connect(&mut app, |message| match message["type"].as_str() {
                Some("start-dev") => Ok(json!({
                    "content": "", "live": true, "sessionId": "a", "workspace": "/repo",
                    "processes": [{"name": "api", "content": "", "live": true}, {"name": "web", "content": "", "live": false}],
                })),
                _ => Ok(json!({})),
            });
            open_with(&mut app, with_processes(), Some("a"));
            app.panes.dev = serde_json::from_value(json!({
                "content": "", "live": true, "sessionId": "a",
                "processes": [{"name": "api", "content": "", "live": true}, {"name": "web", "content": "", "live": false}],
            }))
            .unwrap();
            let processes = app.run_list_processes();
            assert_eq!(
                processes
                    .iter()
                    .map(|row| (row.name.as_str(), row.running))
                    .collect::<Vec<_>>(),
                [("api", true), ("web", false)]
            );
            let text = shown(&render(
                &ActionPickerPaneProps {
                    project: Some(&with_processes()),
                    running: None,
                    dev: Some(DevRow { running: true }),
                    processes: &processes,
                    shells: &[],
                    selected_index: 1,
                },
                100,
                20,
            ));
            assert!(text.contains("Dev          api + web"), "{text}");
            assert!(text.contains("enter show api • x stop api • esc cancel"), "{text}");

            // Rows: Dev, api, web, build. x on the running api asks for a second x, which stops it alone.
            handle_pick_action(&mut app, &Input::text("j"));
            handle_pick_action(&mut app, &Input::text("x"));
            assert_eq!(app.messages.status_message.as_deref(), Some("x again stops api"));
            handle_pick_action(&mut app, &Input::text("x"));
            settle(&mut app, &mut rx).await;
            // Enter on a running process shows it on the Dev tab.
            handle_pick_action(&mut app, &enter());
            assert_eq!(app.mode, Mode::Browse);
            assert_eq!(app.browse.active_tab, RightPaneTab::Dev);
            assert_eq!(app.panes.dev_process.as_deref(), Some("api"));
            app.set_mode(Mode::PickAction);
            // Enter on web (not running) starts it alone and shows the Dev tab.
            handle_pick_action(&mut app, &Input::text("j"));
            handle_pick_action(&mut app, &enter());
            settle(&mut app, &mut rx).await;
            assert_eq!(app.browse.active_tab, RightPaneTab::Dev);
            assert_eq!(app.panes.dev_process.as_deref(), Some("web"));
            let seen = seen.borrow();
            let sent: Vec<(Option<&str>, Option<&str>, Option<bool>)> = seen
                .iter()
                .filter(|message| matches!(message["type"].as_str(), Some("start-dev" | "stop-dev")))
                .map(|message| {
                    (
                        message["type"].as_str(),
                        message["process"].as_str(),
                        message.get("restart").and_then(Value::as_bool),
                    )
                })
                .collect();
            assert_eq!(
                sent,
                [
                    (Some("stop-dev"), Some("api"), None),
                    (Some("start-dev"), Some("web"), None)
                ]
            );
        });
    }

    #[test]
    fn the_shell_row_comes_last_and_says_what_a_restart_stops() {
        let project = trusted();
        let props = ActionPickerPaneProps {
            project: Some(&project),
            running: None,
            dev: Some(DevRow { running: true }),
            processes: &[],
            shells: &[],
            selected_index: 3,
        };
        let text = shown(&render(&props, 100, 24));
        assert!(
            text.contains("lint         make lint ❯ Shell        the worktree's shell · x restarts it + New shell"),
            "{text}"
        );
        assert!(text.contains(
            "Enter shows it. x twice stops the shell and everything running in it, then starts a fresh one. Anything that ignores the hangup is killed 2 s later."
        ));
        assert!(text.contains("Dev and actions keep running."));
        assert!(text.contains("enter show the shell • x restart the shell • esc cancel"));
        let record: ActionRecord = serde_json::from_value(action_record(true)).unwrap_or_default();
        let text = shown(&render(
            &ActionPickerPaneProps {
                running: Some(&record),
                ..props
            },
            100,
            24,
        ));
        // An action still running is named; x on its row stops it (x here acts on the shell).
        assert!(text.contains("build is still running in this worktree: x twice on its row stops it."));
        assert!(text.contains("enter show the shell • x restart the shell • esc cancel"));
        // Without a workspace there is no shell to restart.
        let text = shown(&render(&ActionPickerPaneProps { dev: None, ..props }, 100, 24));
        assert!(!text.contains("Shell"), "{text}");
    }

    #[test]
    fn new_shell_opens_another_and_x_closes_an_extra_one() {
        local(async {
            let (mut app, mut rx) = picker_app();
            let two = json!({
                "content": "$ ", "live": true, "cwd": "/repo", "sessionId": "a", "workspace": "/repo",
                "shells": [{"id": 0, "content": "$ ", "live": true}, {"id": 5, "content": "", "live": true}],
            });
            let one = json!({"content": "$ ", "live": true, "cwd": "/repo", "sessionId": "a", "workspace": "/repo"});
            let seen = connect(&mut app, move |message| match message["type"].as_str() {
                Some("new-terminal") => Ok(two.clone()),
                Some("close-terminal") => Ok(one.clone()),
                other => Err(format!("unexpected {other:?}")),
            });
            open_with(&mut app, trusted(), Some("a"));
            // Dev, build, lint, Shell, + New shell.
            handle_pick_action(&mut app, &Input::text("G"));
            handle_pick_action(&mut app, &enter());
            settle(&mut app, &mut rx).await;
            assert_eq!(app.browse.active_tab, RightPaneTab::Terminal);
            assert_eq!(app.panes.terminal_view, TerminalView::Shell(5));
            assert_eq!(
                app.messages.status_message.as_deref(),
                Some("Opened shell 2 · → types into it, v switches")
            );
            let shells = app.run_list_shells();
            assert_eq!(
                shells
                    .iter()
                    .map(|shell| (shell.id, shell.label.as_str()))
                    .collect::<Vec<_>>(),
                [(0, "Shell 1"), (5, "Shell 2")]
            );
            // Its row says x closes it; x twice does.
            let props = ActionPickerPaneProps {
                project: Some(&trusted()),
                running: None,
                dev: Some(DevRow { running: false }),
                processes: &[],
                shells: &shells,
                selected_index: 4,
            };
            let text = shown(&render(&props, 100, 24));
            assert!(text.contains("Shell 2      x closes it"), "{text}");
            assert!(
                text.contains("enter show shell 2 • x close shell 2 • esc cancel"),
                "{text}"
            );
            app.set_mode(Mode::PickAction);
            app.action_picker.action_index = 4;
            handle_pick_action(&mut app, &Input::text("x"));
            assert_eq!(app.messages.status_message.as_deref(), Some("x again closes shell 2"));
            handle_pick_action(&mut app, &Input::text("x"));
            settle(&mut app, &mut rx).await;
            assert_eq!(app.messages.status_message.as_deref(), Some("Closed shell 2"));
            assert!(app.panes.terminal.shells.is_none());
            let seen = seen.borrow();
            assert_eq!(seen[0]["type"], "new-terminal");
            assert_eq!(
                (&seen[1]["type"], &seen[1]["shell"]),
                (&json!("close-terminal"), &json!(5))
            );
        });
    }

    #[test]
    fn at_most_max_shells_have_a_new_shell_row() {
        let shells: Vec<ShellRow> = (0..MAX_SHELLS)
            .map(|index| ShellRow {
                id: i64::try_from(index).unwrap_or_default(),
                label: format!("Shell {}", index + 1),
            })
            .collect();
        let props = ActionPickerPaneProps {
            project: Some(&trusted()),
            running: None,
            dev: Some(DevRow { running: false }),
            processes: &[],
            shells: &shells,
            selected_index: 0,
        };
        assert!(!shown(&render(&props, 100, 30)).contains("New shell"));
    }

    #[test]
    fn enter_shows_a_shell_and_x_twice_restarts_the_first() {
        local(async {
            let (mut app, mut rx) = picker_app();
            let seen = connect(&mut app, |message| match message["type"].as_str() {
                Some("restart-terminal") => Ok(json!({
                    "content": "$ ", "live": true, "cwd": "/repo", "sessionId": "a", "workspace": "/repo"
                })),
                other => Err(format!("unexpected {other:?}")),
            });
            open_with(&mut app, trusted(), Some("a"));
            app.panes.terminal_view = TerminalView::Action;
            // Dev, build, lint, Shell, + New shell. Enter on the shell shows it and sends nothing.
            handle_pick_action(&mut app, &Input::text("G"));
            handle_pick_action(&mut app, &Input::text("k"));
            handle_pick_action(&mut app, &enter());
            assert_eq!(
                (app.mode, app.browse.active_tab),
                (Mode::Browse, RightPaneTab::Terminal)
            );
            assert_eq!(app.panes.terminal_view, TerminalView::Shell(0));
            assert!(seen.borrow().is_empty() && !app.messages.busy);
            // x twice restarts it.
            app.set_mode(Mode::PickAction);
            app.panes.terminal_view = TerminalView::Action;
            handle_pick_action(&mut app, &Input::text("x"));
            assert!(seen.borrow().is_empty() && !app.messages.busy);
            assert_eq!(
                app.messages.status_message.as_deref(),
                Some("x again restarts the shell")
            );
            handle_pick_action(&mut app, &Input::text("x"));
            assert!(app.messages.busy);
            settle(&mut app, &mut rx).await;
            assert!(!app.messages.busy);
            assert_eq!(app.mode, Mode::Browse);
            assert_eq!(app.browse.active_tab, RightPaneTab::Terminal);
            assert_eq!(app.panes.terminal_view, TerminalView::Shell(0));
            assert!(app.panes.terminal.live && app.panes.terminal.content == "$ ");
            assert_eq!(app.messages.status_message.as_deref(), Some("Restarted the shell"));
            let seen = seen.borrow();
            let layout = app.layout();
            assert_eq!(seen.len(), 1);
            assert_eq!(
                (seen[0]["type"].as_str(), seen[0]["sessionId"].as_str()),
                (Some("restart-terminal"), Some("a"))
            );
            assert_eq!(seen[0]["cols"], i64::from(layout.preview_cols));
            assert_eq!(seen[0]["rows"], i64::from(layout.preview_rows));
        });
    }

    #[test]
    fn a_failed_shell_restart_reports_and_stays_on_the_list() {
        local(async {
            let (mut app, mut rx) = picker_app();
            let _seen = connect(&mut app, |_| {
                Err("Terminal is unavailable: its worktree was deleted".to_owned())
            });
            open_with(&mut app, trusted(), Some("a"));
            handle_pick_action(&mut app, &Input::text("G"));
            handle_pick_action(&mut app, &enter());
            settle(&mut app, &mut rx).await;
            assert_eq!(
                app.messages.error.as_deref(),
                Some("Terminal is unavailable: its worktree was deleted")
            );
            assert_eq!((app.mode, app.messages.busy), (Mode::PickAction, false));
        });
    }

    #[test]
    fn a_failed_run_reports_and_a_mode_change_meanwhile_is_kept() {
        local(async {
            let (mut app, mut rx) = picker_app();
            let _seen = connect(&mut app, |_| Err("another action is running".to_owned()));
            open_with(&mut app, trusted(), Some("a"));
            handle_pick_action(&mut app, &Input::text("j"));
            handle_pick_action(&mut app, &enter());
            settle(&mut app, &mut rx).await;
            assert_eq!(app.messages.error.as_deref(), Some("another action is running"));
            assert_eq!((app.mode, app.messages.busy), (Mode::PickAction, false));
            let _seen = connect(&mut app, watch_and_run_daemon);
            handle_pick_action(&mut app, &enter());
            app.set_mode(Mode::Help);
            settle(&mut app, &mut rx).await;
            assert_eq!(app.mode, Mode::Help);
            assert_eq!(app.browse.active_tab, RightPaneTab::Terminal);
        });
    }

    #[test]
    fn enter_needs_a_session_and_a_connection() {
        local(async {
            let (mut app, _rx) = picker_app();
            open_with(&mut app, trusted(), Some("a"));
            handle_pick_action(&mut app, &Input::text("j"));
            handle_pick_action(&mut app, &enter());
            assert_eq!(
                app.messages.error.as_deref(),
                Some("select a session to run project actions")
            );
            app.messages.error = None;
            let _seen = connect(&mut app, watch_and_run_daemon);
            open_with(&mut app, trusted(), None);
            // Without a session there is no Dev row: row 0 is build.
            handle_pick_action(&mut app, &enter());
            assert_eq!(
                app.messages.error.as_deref(),
                Some("select a session to run project actions")
            );
            assert!(!app.messages.busy);
        });
    }

    #[test]
    fn x_twice_stops_the_running_action_on_its_row_only() {
        local(async {
            let (mut app, mut rx) = picker_app();
            let seen = connect(&mut app, watch_and_run_daemon);
            open_with(&mut app, trusted(), Some("a"));
            // Nothing running: x on a plain action row says so and sends nothing.
            handle_pick_action(&mut app, &Input::text("j"));
            handle_pick_action(&mut app, &Input::text("x"));
            assert!(seen.borrow().is_empty() && !app.messages.busy);
            assert_eq!(app.messages.status_message.as_deref(), Some("Nothing runs on this row"));
            // build runs: x on its row asks again; another key cancels; x twice stops it.
            app.panes.action = serde_json::from_value(action_record(true)).unwrap_or_default();
            handle_pick_action(&mut app, &Input::text("x"));
            assert_eq!(app.messages.status_message.as_deref(), Some("x again stops build"));
            handle_pick_action(&mut app, &Input::text("z"));
            handle_pick_action(&mut app, &Input::text("x"));
            assert!(seen.borrow().is_empty() && !app.messages.busy);
            handle_pick_action(&mut app, &Input::text("x"));
            assert!(app.messages.busy);
            settle(&mut app, &mut rx).await;
            assert!(!app.messages.busy);
            assert_eq!(app.messages.status_message.as_deref(), Some("Stopped build"));
            assert_eq!(app.mode, Mode::PickAction);
            assert_eq!(seen.borrow()[0]["type"], "stop-action");
            // Only its own row stops it: x on the Dev row (Dev stopped) or on lint sends nothing.
            for row in [0, 2] {
                app.action_picker.action_index = row;
                handle_pick_action(&mut app, &Input::text("x"));
                handle_pick_action(&mut app, &Input::text("x"));
            }
            assert_eq!(seen.borrow().len(), 1);
            // An action of another session is not stopped from here.
            app.action_picker.action_index = 1;
            app.panes.action.session_id = Some("other".to_owned());
            handle_pick_action(&mut app, &Input::text("x"));
            handle_pick_action(&mut app, &Input::text("x"));
            assert_eq!(seen.borrow().len(), 1);
        });
    }

    #[test]
    fn a_failed_stop_reports() {
        local(async {
            let (mut app, mut rx) = picker_app();
            let _seen = connect(&mut app, |_| Err("could not stop".to_owned()));
            open_with(&mut app, trusted(), Some("a"));
            app.panes.action = serde_json::from_value(action_record(true)).unwrap_or_default();
            app.action_picker.action_index = 1;
            handle_pick_action(&mut app, &Input::text("x"));
            handle_pick_action(&mut app, &Input::text("x"));
            settle(&mut app, &mut rx).await;
            assert_eq!(app.messages.error.as_deref(), Some("could not stop"));
            assert!(!app.messages.busy);
        });
    }

    fn untrusted_daemon(message: &Value) -> Result<Value, String> {
        match message["type"].as_str() {
            Some("project-info") => Ok(project_info(
                json!({"actions": {"build": "make evil"}}),
                json!({"actions": {"build": "make build"}}),
                false,
                true,
            )),
            Some("run-action") => Ok(action_record(true)),
            other => Err(format!("unexpected {other:?}")),
        }
    }

    fn untrusted_with_fallback() -> ProjectInfo {
        project(
            json!({"actions": {"build": "make evil"}}),
            json!({"actions": {"build": "make build"}}),
            false,
            true,
        )
    }

    #[test]
    fn an_untrusted_action_is_reviewed_first_and_esc_returns_to_the_list() {
        local(async {
            let (mut app, mut rx) = picker_app();
            let seen = connect(&mut app, untrusted_daemon);
            open_with(&mut app, untrusted_with_fallback(), Some("a"));
            handle_pick_action(&mut app, &Input::text("j"));
            handle_pick_action(&mut app, &enter());
            settle(&mut app, &mut rx).await;
            assert_eq!(app.mode, Mode::ReviewProject);
            let review = app.review.review.clone().unwrap_or_else(|| panic!("no review"));
            assert_eq!(review.cwd, "/repo");
            assert_eq!(
                review.purpose.as_deref(),
                Some("About to run the repository action build: make evil")
            );
            let labels = review.labels.unwrap_or_default();
            assert_eq!(labels.enter.map(|part| part.text).as_deref(), Some("enter trust & run"));
            assert_eq!(
                labels.skip.map(|part| part.text).as_deref(),
                Some("s run the global build instead")
            );
            assert!(matches!(review.skip, ReviewSkip::Run(_)));
            // Nothing ran yet.
            assert!(seen.borrow().iter().all(|message| message["type"] != "run-action"));
            crate::app::modes::review::handle_review_project(&mut app, &pressed(|key| key.escape = true));
            assert_eq!(app.mode, Mode::PickAction);
            assert!(app.review.review.is_none());
        });
    }

    #[test]
    fn s_on_the_review_runs_the_global_action_anyway() {
        local(async {
            let (mut app, mut rx) = picker_app();
            let seen = connect(&mut app, untrusted_daemon);
            open_with(&mut app, untrusted_with_fallback(), Some("a"));
            handle_pick_action(&mut app, &Input::text("j"));
            handle_pick_action(&mut app, &enter());
            settle(&mut app, &mut rx).await;
            crate::app::modes::review::handle_review_project(&mut app, &Input::text("s"));
            settle(&mut app, &mut rx).await;
            assert_eq!(app.browse.active_tab, RightPaneTab::Terminal);
            assert_eq!(app.panes.terminal_view, TerminalView::Action);
            assert_eq!(app.mode, Mode::Browse);
            let ran: Vec<Value> = seen
                .borrow()
                .iter()
                .filter(|message| message["type"] == "run-action")
                .cloned()
                .collect();
            assert_eq!(ran.len(), 1);
            assert_eq!(ran[0]["action"], "build");
        });
    }

    #[test]
    fn s_cancels_the_run_when_there_is_no_global_action_to_fall_back_to() {
        local(async {
            let (mut app, mut rx) = picker_app();
            let seen = connect(&mut app, |message| match message["type"].as_str() {
                Some("project-info") => Ok(project_info(
                    json!({"actions": {"test": "echo tests-ok"}}),
                    json!({}),
                    false,
                    true,
                )),
                _ => Ok(action_record(true)),
            });
            open_with(&mut app, untrusted(), Some("a"));
            // Dev, then test.
            handle_pick_action(&mut app, &Input::text("j"));
            handle_pick_action(&mut app, &enter());
            settle(&mut app, &mut rx).await;
            let review = app.review.review.clone().unwrap_or_else(|| panic!("no review"));
            assert!(matches!(review.skip, ReviewSkip::Cancel));
            assert_eq!(
                review
                    .labels
                    .and_then(|labels| labels.skip)
                    .map(|part| part.text)
                    .as_deref(),
                Some("s cancel this run")
            );
            crate::app::modes::review::handle_review_project(&mut app, &Input::text("s"));
            settle(&mut app, &mut rx).await;
            assert_eq!(app.mode, Mode::PickAction);
            assert!(seen.borrow().iter().all(|message| message["type"] != "run-action"));
        });
    }

    #[test]
    fn enter_on_the_trust_review_runs_the_action_with_the_trusted_config() {
        local(async {
            let (mut app, mut rx) = picker_app();
            let seen = connect(&mut app, |message| match message["type"].as_str() {
                Some("project-info") => Ok(project_info(
                    json!({"actions": {"build": "make evil"}}),
                    json!({}),
                    false,
                    true,
                )),
                Some("trust-project") => Ok(project_info(
                    json!({"actions": {"build": "make evil"}}),
                    json!({"actions": {"build": "make evil"}}),
                    true,
                    false,
                )),
                _ => Ok(action_record(true)),
            });
            let project = project(json!({"actions": {"build": "make evil"}}), json!({}), false, true);
            open_with(&mut app, project, Some("a"));
            handle_pick_action(&mut app, &Input::text("j"));
            handle_pick_action(&mut app, &enter());
            settle(&mut app, &mut rx).await;
            assert_eq!(app.mode, Mode::ReviewProject);
            crate::app::modes::review::handle_review_project(&mut app, &enter());
            settle(&mut app, &mut rx).await;
            let types: Vec<String> = seen
                .borrow()
                .iter()
                .map(|message| message["type"].as_str().unwrap_or_default().to_owned())
                .collect();
            assert_eq!(types, ["project-info", "trust-project", "run-action"]);
            assert_eq!(
                app.messages.status_message.as_deref(),
                Some("Repository configuration trusted")
            );
            assert_eq!(app.mode, Mode::Browse);
        });
    }
}
