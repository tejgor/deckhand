//! Port of app.tsx's run list (r, pick-action): the worktree's Dev command first (when the session has a
//! workspace), then the repository's actions; untrusted repository ones are reviewed when chosen. x stops the
//! worktree's running action. ActionPickerPane renders it.

use std::rc::Rc;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use deckhand_client::live::LiveClient;
use deckhand_core::detail_texts::{ProjectAction, project_actions};
use deckhand_core::display::{DetailLine, HintPart, MenuItem, MenuPaneLine, RowStatus, THEME, error_message};
use deckhand_core::jsstr::js_trim;
use deckhand_core::key::Input;
use deckhand_protocol::data::{ActionRecord, ProjectInfo, RightPaneTab};

use crate::app::modes::browse::run_dev;
use crate::app::state::{ActionProject, App, AppAction, AppCallback, Mode, ReviewLabels, ReviewSkip};
use crate::app::{ProjectFn, ReviewOptions, hint_short, truthy};
use crate::keymap::{Binding, HelpRef, KeySpec, Keymap, NamedKey};
use crate::panes::menu::{MenuPaneDetails, MenuPaneProps, render_menu_pane};
use crate::panes::terminal::TerminalView;

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
        action: "stop the worktree's running action (or Dev, on the Dev row)",
        help: HelpRef::Topic("Dev & actions"),
    },
    Binding {
        keys: &[KeySpec::Named(NamedKey::Return)],
        action: "start or stop Dev, or run the selected action",
        help: HelpRef::Topic("Dev & actions"),
    },
    Binding {
        keys: &[KeySpec::Named(NamedKey::Escape)],
        action: "close the run list",
        help: HelpRef::Hidden("the pane's own hint says esc cancels"),
    },
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
    // The picker says whether an action is still running in this worktree (x stops it), whichever tab is shown.
    if let (Some(client), Some(id), true) = (
        app.client(),
        session_id.as_ref().filter(|id| !id.is_empty()),
        truthy(app.selected_workspace().as_ref()),
    ) {
        let layout = app.layout();
        let reply = client.watch_action(Some(id), i64::from(layout.preview_cols), i64::from(layout.preview_rows));
        let task = app.dispatch().spawn(reply, |app: &mut App, result| {
            if let Ok(record) = result
                && record.session_id == app.selected_id_ref()
            {
                app.panes.action = record;
            }
        });
        drop(task);
    }
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
        app.set_mode(Mode::PickAction);
    });
    let options = ReviewOptions {
        required: true,
        gate: Some(Rc::new(|_: &ProjectInfo| false)),
        ..ReviewOptions::default()
    };
    app.review_then(&action_cwd, Some(resume), options);
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
    // Row 0 is the worktree's Dev command when the session has a workspace; the actions follow.
    let dev_row = usize::from(app.run_list_has_dev());
    let rows = dev_row + actions.len();
    let last = rows.saturating_sub(1);
    // The handlers below read the index as it was when the key arrived (React state).
    let index = app.action_picker.action_index;
    if key.escape {
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
    // x stops the worktree's running action (the picker says which) from any row, the Dev row included; one runs
    // at a time per worktree. Without one, x on the Dev row stops Dev.
    if text == "x"
        && let (Some(client), Some(session_id)) = (app.client(), review_session_id.as_ref().filter(|id| !id.is_empty()))
        && app.panes.action.session_id.as_deref() == Some(session_id.as_str())
        && app.panes.action.live
    {
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
        return;
    }
    if dev_row == 1 && index == 0 {
        let selected = app.selected_session();
        let dev_runs = selected
            .as_ref()
            .is_some_and(|session| session.dev_running == Some(true))
            || (app.panes.dev.session_id.as_deref() == selected.as_ref().map(|session| session.id.as_str())
                && app.panes.dev.live);
        if key.return_ || (text == "x" && dev_runs) {
            let back: AppCallback = Rc::new(|app: &mut App| app.set_mode(Mode::PickAction));
            run_dev(app, Some(back));
        }
        return;
    }
    let picked = index.checked_sub(dev_row).and_then(|row| actions.get(row));
    if key.return_
        && let (Some(picked), Some(action_project)) = (picked, action_project.as_ref())
    {
        run_action_row(app, picked, action_project);
    }
}

/// TS: ActionPickerPane `dev`: the Dev row and whether it runs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DevRow {
    pub running: bool,
}

/// TS: app.tsx ActionPickerPane props (width and height are the area's).
#[derive(Clone, Copy, Debug)]
pub struct ActionPickerPaneProps<'a> {
    pub project: Option<&'a ProjectInfo>,
    /// The worktree's last action, when it belongs to the picker's session.
    pub running: Option<&'a ActionRecord>,
    pub dev: Option<DevRow>,
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

/// TS: app.tsx ActionPickerPane (a MenuPane).
pub fn render_action_picker_pane(props: &ActionPickerPaneProps<'_>, buf: &mut Buffer, area: Rect) {
    let project = props.project;
    let actions = project_actions(project);
    let dev_command = project
        .and_then(|project| project.effective.dev_command.as_deref())
        .map(js_trim)
        .filter(|command| !command.is_empty())
        .unwrap_or("dev")
        .to_owned();
    let dev_offset = usize::from(props.dev.is_some());
    let on_dev = props.dev.is_some() && props.selected_index == 0;
    let selected = props
        .selected_index
        .checked_sub(dev_offset)
        .and_then(|row| actions.get(row));
    let untrusted_dev = props.dev.is_some()
        && project.is_some_and(|project| project.needs_review && truthy(project.project.config.dev_command.as_ref()));
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
    items.extend(actions.iter().map(|action| MenuItem {
        key: action.name.clone(),
        label: action.name.clone(),
        description: Some(action.command.clone()),
        status: action.needs_trust.then(|| RowStatus {
            text: "· needs trust".to_owned(),
            color: Some(THEME.warn),
        }),
        ..MenuItem::default()
    }));

    let details = if on_dev {
        let running = props.dev.is_some_and(|dev| dev.running);
        let mut lines = vec![
            command_line(dev_command.clone()),
            DetailLine {
                text: if running {
                    format!(
                        "Running in this worktree: Enter {}stops it.",
                        if running_name.is_some() { "" } else { "(or x) " }
                    )
                } else {
                    "Enter starts it in this worktree; its output is on the Dev tab, shared by every session there."
                        .to_owned()
                },
                ..DetailLine::default()
            },
        ];
        if let Some(name) = &running_name {
            lines.push(colored(
                format!("{name} is still running in this worktree: x stops it."),
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
                Some(name) => colored(
                    format!("{name} is still running in this worktree: x stops it, then run another."),
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

    let mut hint: Vec<HintPart> = vec![HintPart::from(if on_dev {
        if props.dev.is_some_and(|dev| dev.running) {
            "enter stop Dev"
        } else {
            "enter start Dev"
        }
    } else {
        "enter run"
    })];
    if let Some(name) = &running_name {
        hint.push(HintPart::from(format!("x stop {name}").as_str()));
    }
    hint.push(HintPart::from("esc cancel"));

    let pane = MenuPaneProps {
        title: "Run",
        subtitle: &subtitle,
        items: &items,
        selected: props.selected_index,
        details: details.as_ref(),
        empty: Some("No actions configured (C → Settings › Actions adds one)"),
        hint: &hint,
        label_width: None,
    };
    render_menu_pane(&pane, buf, area);
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

    fn shown(rows: &[String]) -> String {
        rows.join("\n")
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
    fn the_run_list_renders_like_the_golden() {
        let path = format!("{}/../../fixtures/ui/43-run-list.txt", env!("CARGO_MANIFEST_DIR"));
        let golden = std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{path}: {error}"));
        // The right pane starts at column 32 and runs from row 2 to the row with its bottom border.
        let lines: Vec<Vec<char>> = golden.lines().map(|line| line.chars().collect()).collect();
        let right = |line: &Vec<char>| line.iter().skip(32).collect::<String>().trim_end().to_owned();
        let end = lines
            .iter()
            .position(|line| line.get(32) == Some(&'╰'))
            .unwrap_or_else(|| panic!("no pane bottom in the golden"));
        let want: Vec<String> = lines[2..=end].iter().map(right).collect();
        let project = untrusted();
        let props = ActionPickerPaneProps {
            project: Some(&project),
            running: None,
            dev: Some(DevRow { running: false }),
            selected_index: 0,
        };
        let got = render(&props, 98, u16::try_from(want.len()).unwrap_or(0));
        assert_eq!(got.len(), want.len());
        for (index, (got, want)) in got.iter().zip(&want).enumerate() {
            assert_eq!(got, want, "row {index}");
        }
    }

    #[test]
    fn the_pane_lists_dev_then_the_actions_with_the_selected_ones_details() {
        let project = trusted();
        let props = ActionPickerPaneProps {
            project: Some(&project),
            running: None,
            dev: Some(DevRow { running: true }),
            selected_index: 0,
        };
        let text = shown(&render(&props, 100, 24));
        assert!(text.contains("Dev, then your actions (global defaults and repository)"));
        assert!(text.contains("Dev    make dev") && text.contains("● running"));
        assert!(text.contains("build  make build"));
        assert!(text.contains("Running in this worktree: Enter (or x) stops it."));
        assert!(text.contains("enter stop Dev • esc cancel"));
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
    fn a_running_action_is_named_and_x_stops_it() {
        let project = trusted();
        let record: ActionRecord = serde_json::from_value(action_record(true)).unwrap_or_default();
        let on_dev = ActionPickerPaneProps {
            project: Some(&project),
            running: Some(&record),
            dev: Some(DevRow { running: false }),
            selected_index: 0,
        };
        let text = shown(&render(&on_dev, 100, 24));
        assert!(text.contains("build is still running in this worktree: x stops it."));
        assert!(text.contains("enter start Dev • x stop build • esc cancel"));
        let on_action = ActionPickerPaneProps {
            selected_index: 2,
            ..on_dev
        };
        let text = shown(&render(&on_action, 100, 24));
        assert!(text.contains("build is still running in this worktree: x stops it, then run another."));
        // Dev running beside a running action: Enter alone stops Dev.
        let both = ActionPickerPaneProps {
            dev: Some(DevRow { running: true }),
            ..on_dev
        };
        assert!(shown(&render(&both, 100, 24)).contains("Running in this worktree: Enter stops it."));
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
        assert!(shown(&render(&unnamed, 100, 24)).contains("x stop action"));
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
        // Dev, build, lint.
        for (input, want) in [("j", 1), ("j", 2), ("j", 2), ("k", 1), ("g", 0), ("k", 0), ("G", 2)] {
            handle_pick_action(&mut app, &Input::text(input));
            assert_eq!(app.action_picker.action_index, want, "{input}");
        }
        handle_pick_action(&mut app, &pressed(|key| key.up_arrow = true));
        assert_eq!(app.action_picker.action_index, 1);
        handle_pick_action(&mut app, &pressed(|key| key.down_arrow = true));
        assert_eq!(app.action_picker.action_index, 2);
        handle_pick_action(&mut app, &pressed(|key| key.home = true));
        assert_eq!(app.action_picker.action_index, 0);
        handle_pick_action(&mut app, &pressed(|key| key.end = true));
        assert_eq!(app.action_picker.action_index, 2);
        handle_pick_action(&mut app, &pressed(|key| key.escape = true));
        assert_eq!(app.mode, Mode::Browse);
        // A session without a workspace has no Dev row: two rows.
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
    fn x_stops_the_worktrees_running_action_from_any_row() {
        local(async {
            let (mut app, mut rx) = picker_app();
            let seen = connect(&mut app, watch_and_run_daemon);
            open_with(&mut app, trusted(), Some("a"));
            // Nothing running: x on a plain action row does nothing.
            handle_pick_action(&mut app, &Input::text("j"));
            handle_pick_action(&mut app, &Input::text("x"));
            assert!(seen.borrow().is_empty() && !app.messages.busy);
            app.panes.action = serde_json::from_value(action_record(true)).unwrap_or_default();
            handle_pick_action(&mut app, &Input::text("x"));
            assert!(app.messages.busy);
            settle(&mut app, &mut rx).await;
            assert!(!app.messages.busy);
            assert_eq!(app.messages.status_message.as_deref(), Some("Stopped build"));
            assert_eq!(app.mode, Mode::PickAction);
            assert_eq!(seen.borrow()[0]["type"], "stop-action");
            // On the Dev row too, and a nameless action is "the action".
            app.action_picker.action_index = 0;
            app.panes.action.name = None;
            handle_pick_action(&mut app, &Input::text("x"));
            settle(&mut app, &mut rx).await;
            assert_eq!(app.messages.status_message.as_deref(), Some("Stopped the action"));
            // An action of another session is not stopped from here.
            app.panes.action.session_id = Some("other".to_owned());
            let before = seen.borrow().len();
            handle_pick_action(&mut app, &Input::text("x"));
            assert_eq!(seen.borrow().len(), before);
        });
    }

    #[test]
    fn a_failed_stop_reports() {
        local(async {
            let (mut app, mut rx) = picker_app();
            let _seen = connect(&mut app, |_| Err("could not stop".to_owned()));
            open_with(&mut app, trusted(), Some("a"));
            app.panes.action = serde_json::from_value(action_record(true)).unwrap_or_default();
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
