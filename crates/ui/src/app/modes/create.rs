//! New-session flow (n / N, n on a task): pick-program (agent, fork, from handoff), enter-name (name, workspace, base
//! branch) and pick-worktree (an existing worktree, searchable), the CreatePane and WorktreePickerPane, and creating
//! the session (reviewing an untrusted repository's creation parts first).

use std::rc::Rc;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use deckhand_core::display::{HINT_SEPARATOR, HintPart, MenuItem, compact_path, fit_hint, truncate};
use deckhand_core::jsstr::{js_len, js_slice, js_split_whitespace, js_to_lower, js_trim};
use deckhand_core::key::Input;
use deckhand_protocol::data::{
    CreateSessionInput, ProgramKey, ProjectInfo, SessionRecord, SubSessionKind, Task, WorktreeInfoRecord, WorktreeMode,
};

use crate::app::state::{App, Mode, ReviewLabels, ReviewSkip, TaskStart};
use crate::app::{
    PROGRAMS, ReviewOptions, WORKTREE_MODES, base_options, fork_stays_in_parent, hint_short, sanitize_name_input,
    session_title_from_task, supports_forked_sub_session, truthy,
};
use crate::keymap::{Binding, HelpRef, KeySpec, Keymap, NamedKey};
use crate::panes::menu::{MenuListProps, SelectableRowProps, menu_list, selectable_row};
use crate::primitives::{Border, Row, Seg, TextStyle, Wrap, bordered, draw_rows, fit_row, truncate_row, with_base};
use crate::theme::THEME;

/// pick-program keys.
pub static PICK_PROGRAM_KEYMAP: Keymap = &[
    Binding {
        keys: &[KeySpec::Named(NamedKey::Up), KeySpec::Char("k")],
        action: "previous agent or option",
        help: HelpRef::Topic("Start here"),
    },
    Binding {
        keys: &[KeySpec::Named(NamedKey::Down), KeySpec::Char("j")],
        action: "next agent or option",
        help: HelpRef::Topic("Start here"),
    },
    Binding {
        keys: &[KeySpec::Named(NamedKey::Return)],
        action: "continue (fork / from handoff: pick its agent)",
        help: HelpRef::Topic("Start here"),
    },
    Binding {
        keys: &[KeySpec::Named(NamedKey::Escape)],
        action: "cancel (from handoff: back to the choices)",
        help: HelpRef::Topic("Start here"),
    },
];

/// enter-name keys.
pub static ENTER_NAME_KEYMAP: Keymap = &[
    Binding {
        keys: &[KeySpec::Named(NamedKey::Tab)],
        action: "next workspace (no / new / existing worktree)",
        help: HelpRef::Topic("Start here"),
    },
    Binding {
        keys: &[KeySpec::Named(NamedKey::Up), KeySpec::Named(NamedKey::Down)],
        action: "a new worktree's base",
        help: HelpRef::Topic("Start here"),
    },
    Binding {
        keys: &[KeySpec::Named(NamedKey::Return)],
        action: "create (existing worktree: pick it)",
        help: HelpRef::Topic("Start here"),
    },
    Binding {
        keys: &[KeySpec::Named(NamedKey::Backspace), KeySpec::Named(NamedKey::Delete)],
        action: "delete a typed character",
        help: HelpRef::Hidden("editing the typed name"),
    },
    Binding {
        keys: &[KeySpec::Named(NamedKey::Escape)],
        action: "back to the agent",
        help: HelpRef::Topic("Start here"),
    },
];

/// pick-worktree keys.
pub static PICK_WORKTREE_KEYMAP: Keymap = &[
    Binding {
        keys: &[KeySpec::Named(NamedKey::Up), KeySpec::Char("k")],
        action: "previous worktree",
        help: HelpRef::Topic("Start here"),
    },
    Binding {
        keys: &[KeySpec::Named(NamedKey::Down), KeySpec::Char("j")],
        action: "next worktree",
        help: HelpRef::Topic("Start here"),
    },
    Binding {
        keys: &[KeySpec::Char("/")],
        action: "search the worktrees (enter keeps it, esc clears)",
        help: HelpRef::Topic("Find & organize"),
    },
    Binding {
        keys: &[KeySpec::Named(NamedKey::Return)],
        action: "create in the selected worktree",
        help: HelpRef::Topic("Start here"),
    },
    Binding {
        keys: &[KeySpec::Named(NamedKey::Escape)],
        action: "clear the search, else back to the form",
        help: HelpRef::Topic("Start here"),
    },
];

/// TS `sessions.find(session => session.id === createParentId)` (only with a parent ID).
fn create_parent(app: &App) -> Option<SessionRecord> {
    let id = app.create.parent_id.as_ref().filter(|id| !id.is_empty())?;
    app.sessions().into_iter().find(|session| &session.id == id)
}

/// TS `Math.max(0, PROGRAMS.findIndex(program => program.key === key))`.
fn program_position(key: ProgramKey) -> usize {
    PROGRAMS.iter().position(|program| program.key == key).unwrap_or(0)
}

/// TS: n / N in browse: the form (children keep their parent's agent and directory; the repository's defaults
/// preselect the picker).
pub fn open_new_session(app: &mut App, sub_session: bool) {
    // No review here: the repository's defaults only preselect the picker (trusted or not). Creating a new worktree
    // reviews whatever it would run (see confirm_create).
    let parent = if sub_session { app.selected_session() } else { None };
    let target_cwd = parent
        .as_ref()
        .map_or_else(|| app.props.cwd.clone(), |parent| parent.cwd.clone());
    let resume = Rc::new(move |app: &mut App, project: Option<ProjectInfo>| {
        let defaults = if parent.is_some() {
            None
        } else {
            project.map(|project| project.effective.into_inner())
        };
        app.create.handoff_from_id = None;
        app.browse.session_filter = deckhand_protocol::records::SessionFilter::Active;
        app.browse.session_query.clear();
        let program = parent
            .as_ref()
            .map(|parent| parent.program)
            .or_else(|| defaults.as_ref().and_then(|defaults| defaults.default_agent))
            .unwrap_or(ProgramKey::Claude);
        app.create.program_index = program_position(program);
        app.create.draft_name.clear();
        app.create.task_start = None;
        app.create.branch_list = None;
        app.create.base_index = 0;
        app.create.parent_id = parent.as_ref().map(|parent| parent.id.clone());
        app.create.sub_session_kind = parent.as_ref().map(|_| SubSessionKind::Clean);
        app.create.worktree_mode = defaults
            .as_ref()
            .and_then(|defaults| defaults.default_workspace)
            .unwrap_or(WorktreeMode::None);
        app.set_mode(Mode::PickProgram);
    });
    let options = ReviewOptions {
        gate: Some(Rc::new(|_: &ProjectInfo| false)),
        ..ReviewOptions::default()
    };
    app.review_then(&target_cwd, Some(resume), options);
}

/// TS `startFromTask` (n on a backlog task): the form filled in from the task, in a new worktree by default.
pub fn start_from_task(app: &mut App, task: &Task) {
    // The session it creates is linked to the task, whose text is typed (not sent) into its agent's input.
    let start = TaskStart {
        id: task.id.clone(),
        title: task.title.clone(),
    };
    let target_cwd = app.props.cwd.clone();
    let resume = Rc::new(move |app: &mut App, project: Option<ProjectInfo>| {
        let defaults = project.map(|project| project.effective.into_inner());
        app.create.handoff_from_id = None;
        app.create.parent_id = None;
        app.create.sub_session_kind = None;
        app.browse.session_filter = deckhand_protocol::records::SessionFilter::Active;
        app.browse.session_query.clear();
        let program = defaults
            .as_ref()
            .and_then(|defaults| defaults.default_agent)
            .unwrap_or(ProgramKey::Claude);
        app.create.program_index = program_position(program);
        app.create.draft_name = session_title_from_task(&start.title);
        app.create.task_start = Some(start.clone());
        app.create.branch_list = None;
        app.create.base_index = 0;
        app.create.worktree_mode = WorktreeMode::New;
        app.set_mode(Mode::PickProgram);
    });
    let options = ReviewOptions {
        gate: Some(Rc::new(|_: &ProjectInfo| false)),
        ..ReviewOptions::default()
    };
    app.review_then(&target_cwd, Some(resume), options);
}

/// TS: the pick-program branch of the useTerminalInput handler.
pub fn handle_pick_program(app: &mut App, input: &Input) {
    let key = &input.key;
    if key.escape {
        // Back from "from its handoff" to the sub-session choices first.
        if truthy(app.create.handoff_from_id.as_ref()) && truthy(app.create.parent_id.as_ref()) {
            app.create.handoff_from_id = None;
            app.create.program_index = 0;
            return;
        }
        app.create.parent_id = None;
        app.create.sub_session_kind = None;
        if app.create.task_start.is_some() {
            app.create.task_start = None;
            app.set_mode(Mode::Tasks);
            return;
        }
        app.set_mode(Mode::Browse);
        return;
    }
    let parent = create_parent(app);
    let no_handoff = !truthy(app.create.handoff_from_id.as_ref());
    // After the agents: ⑂ fork the parent (when it can be), then ↳ from its handoff (after H exported one).
    let fork_option = parent.is_some() && no_handoff && supports_forked_sub_session(parent.as_ref());
    let handoff_option = parent
        .as_ref()
        .is_some_and(|parent| truthy(parent.handoff_path.as_ref()))
        && no_handoff;
    let handoff_index = PROGRAMS.len() + usize::from(fork_option);
    let option_count = PROGRAMS.len() + usize::from(fork_option) + usize::from(handoff_option);
    let index = app.create.program_index;
    if key.up_arrow || input.input == "k" {
        app.create.program_index = (index % option_count + option_count - 1) % option_count;
        return;
    }
    if key.down_arrow || input.input == "j" {
        app.create.program_index = (index + 1) % option_count;
        return;
    }
    if key.return_
        && handoff_option
        && let Some(parent) = parent.as_ref()
        && index == handoff_index
    {
        // A clean child that starts from the exported handoff document; the picker now asks for its agent.
        app.create.handoff_from_id = Some(parent.id.clone());
        app.create.sub_session_kind = Some(SubSessionKind::Clean);
        app.create.worktree_mode = WorktreeMode::None;
        app.create.program_index = program_position(parent.program);
        return;
    }
    if key.return_ {
        match parent.as_ref() {
            Some(parent) if fork_option && index == PROGRAMS.len() => {
                app.create.sub_session_kind = Some(SubSessionKind::Forked);
                app.create.program_index = program_position(parent.program);
                if fork_stays_in_parent(Some(parent.program), Some(SubSessionKind::Forked)) {
                    app.create.worktree_mode = WorktreeMode::None;
                }
            }
            _ => app.create.sub_session_kind = parent.as_ref().map(|_| SubSessionKind::Clean),
        }
        app.set_mode(Mode::EnterName);
    }
}

/// The agent picked in the form (none when the index is on the fork / handoff rows).
fn picked_program(app: &App) -> Option<ProgramKey> {
    PROGRAMS.get(app.create.program_index).map(|program| program.key)
}

/// TS: the enter-name branch.
pub fn handle_enter_name(app: &mut App, input: &Input) {
    let key = &input.key;
    if key.escape {
        app.set_mode(Mode::PickProgram);
        return;
    }
    if key.return_ {
        match app.create.worktree_mode {
            WorktreeMode::Existing => list_existing_worktrees(app),
            WorktreeMode::New => confirm_create(app),
            WorktreeMode::None => submit_create(app, None, None),
        }
        return;
    }
    if key.backspace || key.delete {
        app.create.draft_name = js_slice(&app.create.draft_name, 0, Some(-1));
        return;
    }
    let stays_in_parent = fork_stays_in_parent(picked_program(app), app.create.sub_session_kind);
    if key.tab {
        if stays_in_parent {
            return;
        }
        let current = app.create.worktree_mode;
        let index = WORKTREE_MODES.iter().position(|item| item.key == current);
        // TS findIndex gives -1 for an unknown mode, so the next one is the first.
        let next = index.map_or(0, |index| (index + 1) % WORKTREE_MODES.len());
        app.create.worktree_mode = WORKTREE_MODES[next].key;
        return;
    }
    if (key.up_arrow || key.down_arrow) && app.create.worktree_mode == WorktreeMode::New && !stays_in_parent {
        let count = base_options(app.create.branch_list.as_ref()).len();
        if count > 1 {
            let step = if key.down_arrow { 1 } else { count - 1 };
            app.create.base_index = (app.create.base_index + step) % count;
        }
        return;
    }
    if key.ctrl || key.meta || key.up_arrow || key.down_arrow || key.left_arrow || key.right_arrow {
        return;
    }
    if !input.input.is_empty() {
        let text = sanitize_name_input(&input.input);
        if !text.is_empty() {
            app.create.draft_name.push_str(&text);
        }
    }
}

/// TS: Enter in enter-name with an existing worktree: list the repository's worktrees, then pick one.
fn list_existing_worktrees(app: &mut App) {
    if js_trim(&app.create.draft_name).is_empty() {
        app.set_error(Some("title cannot be empty".to_owned()));
        return;
    }
    let Some(client) = app.client() else {
        app.set_error(Some("still connecting to daemon".to_owned()));
        return;
    };
    let target_cwd = create_parent(app).map_or_else(|| app.props.cwd.clone(), |parent| parent.cwd);
    app.set_busy(true);
    let reply = client.list_worktrees(&target_cwd);
    let task = app.dispatch().spawn(reply, |app: &mut App, result| {
        match result {
            Ok(items) => {
                app.create.worktrees = items;
                app.create.worktree_query.clear();
                app.create.worktree_typing = false;
                app.create.worktree_index = 0;
                app.set_mode(Mode::PickWorktree);
            }
            Err(error) => app.set_error(Some(error.to_string())),
        }
        app.set_busy(false);
    });
    drop(task);
}

/// TS: the pick-worktree branch.
pub fn handle_pick_worktree(app: &mut App, input: &Input) {
    let key = &input.key;
    let typing = app.create.worktree_typing;
    if key.up_arrow || (!typing && input.input == "k") {
        app.create.worktree_index = app.create.worktree_index.saturating_sub(1);
        return;
    }
    if key.down_arrow || (!typing && input.input == "j") {
        let count = filtered_worktrees(&app.create.worktrees, &app.create.worktree_query).len();
        app.create.worktree_index = count.saturating_sub(1).min(app.create.worktree_index + 1);
        return;
    }
    // Searching (/): keys are text until Enter keeps the search or Esc clears it, as in the sidebar's search.
    if typing {
        if key.escape {
            app.create.worktree_query.clear();
            app.create.worktree_typing = false;
            app.create.worktree_index = 0;
            return;
        }
        if key.return_ {
            app.create.worktree_typing = false;
            return;
        }
        if key.backspace || key.delete {
            app.create.worktree_query = js_slice(&app.create.worktree_query, 0, Some(-1));
            app.create.worktree_index = 0;
            return;
        }
        if key.ctrl || key.meta || key.left_arrow || key.right_arrow || key.tab {
            return;
        }
        let text = sanitize_name_input(&input.input);
        if !text.is_empty() {
            app.create.worktree_query.push_str(&text);
            app.create.worktree_index = 0;
        }
        return;
    }
    if key.escape {
        if app.create.worktree_query.is_empty() {
            app.set_mode(Mode::EnterName);
        } else {
            app.create.worktree_query.clear();
            app.create.worktree_index = 0;
        }
        return;
    }
    if input.input == "/" {
        app.create.worktree_typing = true;
        return;
    }
    if key.return_ {
        let path = filtered_worktrees(&app.create.worktrees, &app.create.worktree_query)
            .get(app.create.worktree_index)
            .map(|worktree| worktree.path.clone());
        if let Some(path) = path {
            submit_create(app, Some(path), None);
        }
    }
}

/// TS `untrustedCreationParts`: what creating a new worktree would take from an untrusted repository config.
pub fn untrusted_creation_parts(project: &ProjectInfo) -> Vec<String> {
    let config = &*project.project.config;
    let mut parts = Vec::new();
    if let Some(setup) = config.setup_command.as_deref().filter(|setup| !setup.is_empty()) {
        parts.push(format!("setupCommand ({setup})"));
    }
    if let Some(hook) = project.project.creation_hook.as_ref() {
        parts.push(format!("creation hook ({})", hook.file));
    }
    if let Some(worktree) = config.worktree.as_ref() {
        let fields: Vec<&str> = [
            ("location", worktree.location.is_some()),
            ("branchFrom", worktree.branch_from.is_some()),
            ("branchName", worktree.branch_name.is_some()),
            ("symlink", worktree.symlink.is_some()),
            ("files", worktree.files.is_some()),
        ]
        .into_iter()
        .filter_map(|(field, set)| set.then_some(field))
        .collect();
        if !fields.is_empty() {
            parts.push(format!("worktree settings ({})", fields.join(", ")));
        }
    }
    parts
}

/// TS `confirmCreate`: create, reviewing the untrusted parts a new worktree would use first.
pub fn confirm_create(app: &mut App) {
    // The review shows right before creating; s creates with global settings only, Esc returns to the form.
    if js_trim(&app.create.draft_name).is_empty() {
        app.set_error(Some("title cannot be empty".to_owned()));
        return;
    }
    let target_cwd = create_parent(app).map_or_else(|| app.props.cwd.clone(), |parent| parent.cwd);
    let labels = ReviewLabels {
        enter: Some(hint_short("enter trust & create", "enter trust")),
        skip: Some(hint_short(
            "s create without the repo's settings (global only)",
            "s without them",
        )),
    };
    let options = ReviewOptions {
        gate: Some(Rc::new(|project: &ProjectInfo| {
            project.needs_review && !untrusted_creation_parts(project).is_empty()
        })),
        required: false,
        back: Some(Rc::new(|app: &mut App| app.set_mode(Mode::EnterName))),
        skip: ReviewSkip::Run(Rc::new(|app: &mut App, project: ProjectInfo| {
            submit_create(app, None, Some(project.project.fingerprint));
        })),
        labels: Some(Rc::new(move |_: &ProjectInfo| labels.clone())),
        purpose: Some(Rc::new(|project: &ProjectInfo| {
            format!(
                "About to create a new worktree using this repository's {}.",
                untrusted_creation_parts(project).join(", ")
            )
        })),
    };
    let resume = Rc::new(|app: &mut App, _: Option<ProjectInfo>| submit_create(app, None, None));
    app.review_then(&target_cwd, Some(resume), options);
}

/// TS `submitCreate` (`project_fingerprint`: the repository config reviewed and skipped).
pub fn submit_create(app: &mut App, existing_worktree_path: Option<String>, project_fingerprint: Option<String>) {
    let title = js_trim(&app.create.draft_name).to_owned();
    if title.is_empty() {
        app.set_error(Some("title cannot be empty".to_owned()));
        return;
    }
    let Some(client) = app.client() else {
        app.set_error(Some("still connecting to daemon".to_owned()));
        return;
    };
    app.set_busy(true);
    app.set_error(None);
    let Some(program) = picked_program(app) else {
        // TS: `PROGRAMS[programIndex]!.key` throws inside the try.
        app.set_error(Some("Cannot read properties of undefined (reading 'key')".to_owned()));
        app.set_busy(false);
        return;
    };
    let parent = create_parent(app);
    let layout = app.layout();
    let create = &app.create;
    let has_parent = truthy(create.parent_id.as_ref());
    let base_branch = if create.worktree_mode == WorktreeMode::New {
        base_options(create.branch_list.as_ref())
            .get(create.base_index)
            .and_then(|option| option.value.clone())
            .filter(|value| !value.is_empty())
    } else {
        None
    };
    let input = CreateSessionInput {
        title,
        program,
        cwd: parent
            .as_ref()
            .map_or_else(|| app.props.cwd.clone(), |parent| parent.cwd.clone()),
        repo_root: parent
            .as_ref()
            .map_or_else(|| app.props.repo_root.clone(), |parent| parent.repo_root.clone()),
        cols: i64::from(layout.preview_cols),
        rows: i64::from(layout.preview_rows),
        worktree_mode: Some(create.worktree_mode),
        existing_worktree_path,
        parent_session_id: create.parent_id.clone(),
        sub_session_kind: if has_parent {
            Some(create.sub_session_kind.unwrap_or(SubSessionKind::Clean))
        } else {
            None
        },
        handoff_from_session_id: create.handoff_from_id.clone(),
        base_branch,
        task_id: create
            .task_start
            .as_ref()
            .filter(|_| !has_parent)
            .map(|task| task.id.clone()),
        project_fingerprint,
    };
    let reply = client.create_session(input);
    let task = app.dispatch().spawn(reply, |app: &mut App, result| {
        match result {
            Ok(created) => {
                app.create.draft_name.clear();
                app.create.task_start = None;
                app.create.parent_id = None;
                app.create.sub_session_kind = None;
                app.create.handoff_from_id = None;
                app.create.worktree_mode = WorktreeMode::None;
                app.set_mode(Mode::Browse);
                app.set_selected_id(Some(created.id.clone()));
                app.upsert(created);
            }
            Err(error) => app.set_error(Some(error.to_string())),
        }
        app.set_busy(false);
    });
    drop(task);
}

/// TS: the base-branches effect: loaded once the form shows a new worktree.
pub fn load_branches(app: &mut App) {
    let Some(client) = app.client() else {
        return;
    };
    let target_cwd = create_parent(app).map_or_else(|| app.props.cwd.clone(), |parent| parent.cwd);
    // The effect's `cancelled` flag: a later run (or the effect ending) bumps the generation.
    let generation = app.effects.branches.generation;
    let reply = client.list_branches(&target_cwd);
    let task = app.dispatch().spawn(reply, move |app: &mut App, result| {
        if app.effects.branches.generation != generation {
            return;
        }
        // A failed listing leaves "loading branches…" (TS `.catch(() => {})`).
        if let Ok(list) = result {
            app.create.branch_list = Some(list);
        }
    });
    app.effects.branches.task = Some(task);
}

/// TS `filteredWorktrees`: every search term in the branch or path (case-insensitive).
pub fn filtered_worktrees<'a>(worktrees: &'a [WorktreeInfoRecord], query: &str) -> Vec<&'a WorktreeInfoRecord> {
    let lowered = js_to_lower(query);
    let terms = js_split_whitespace(js_trim(&lowered));
    if terms.is_empty() {
        return worktrees.iter().collect();
    }
    worktrees
        .iter()
        .filter(|worktree| {
            let haystack = js_to_lower(&format!("{} {}", worktree.branch, worktree.path));
            terms.iter().all(|term| haystack.contains(term))
        })
        .collect()
}

/// TS `worktreeLabel` (`main · branch  path`).
pub fn worktree_label(worktree: &WorktreeInfoRecord, width: usize) -> String {
    let branch = if worktree.branch.is_empty() {
        "(detached)"
    } else {
        worktree.branch.as_str()
    };
    let prefix = if worktree.is_main { "main · " } else { "" };
    let width_js = i64::try_from(width).unwrap_or(i64::MAX);
    let used = i64::try_from(js_len(prefix) + js_len(branch)).unwrap_or(i64::MAX);
    let path_budget = usize::try_from(8.max(width_js.saturating_sub(used).saturating_sub(2))).unwrap_or(8);
    truncate(
        &format!("{prefix}{branch}  {}", compact_path(&worktree.path, path_budget)),
        width,
    )
}

/// TS: the CreatePane `mode`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CreateStep {
    PickProgram,
    EnterName,
}

/// TS: CreatePane `base`: the option shown, its position and the number of options.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BaseShown {
    pub label: String,
    pub index: usize,
    pub count: usize,
}

/// TS: app.tsx CreatePane props (width is the area's).
#[derive(Clone, Copy, Debug)]
pub struct CreatePaneProps<'a> {
    pub mode: CreateStep,
    pub program_index: usize,
    pub draft_name: &'a str,
    pub worktree_mode: WorktreeMode,
    pub parent_title: Option<&'a str>,
    pub parent_workspace_label: Option<&'a str>,
    pub sub_session_kind: Option<SubSessionKind>,
    pub show_fork_option: bool,
    /// ↳ From handoff: the parent has an exported handoff (H).
    pub show_handoff_option: bool,
    /// The child starts from the parent's handoff; only its agent is left to pick.
    pub from_handoff: bool,
    /// Started from this task (n on the board).
    pub task_title: Option<&'a str>,
    pub base: Option<&'a BaseShown>,
}

/// A `<Text>` (Ink's default `wrap="wrap"`) of `segs` under `base`, wrapped to `width`.
fn wrapped(segs: Row, base: TextStyle, width: usize) -> Vec<Row> {
    fit_row(&with_base(segs, base), width, Wrap::Wrap)
}

/// A `<Text wrap="truncate-end">` of `segs` under `base`, cut to `width`.
fn truncated(segs: Row, base: TextStyle, width: usize) -> Row {
    truncate_row(&with_base(segs, base), width, Wrap::TruncateEnd)
}

/// A muted hint line (`fitHint` of `parts`).
fn hint_row(parts: &[HintPart], width: usize) -> Row {
    truncated(
        vec![Seg::plain(fit_hint(parts, width, HINT_SEPARATOR))],
        TextStyle::fg(THEME.muted),
        width,
    )
}

/// TS: app.tsx CreatePane's rows inside its border.
pub fn create_pane_rows(props: &CreatePaneProps<'_>, width: usize) -> Vec<Row> {
    let pick = props.mode == CreateStep::PickProgram;
    let fork_selected = pick && props.show_fork_option && props.program_index == PROGRAMS.len();
    let handoff_selected = pick
        && props.show_handoff_option
        && props.program_index == PROGRAMS.len() + usize::from(props.show_fork_option);
    let content_width = width.saturating_sub(4).max(1);
    let program = PROGRAMS.get(props.program_index);
    let program_label = program.map_or("", |program| program.label);
    let stays_in_parent = fork_stays_in_parent(program.map(|program| program.key), props.sub_session_kind);
    let parent_title = props.parent_title.filter(|title| !title.is_empty());
    let workspace_label = match props.parent_workspace_label {
        Some(label) if !label.is_empty() && (props.worktree_mode == WorktreeMode::None || stays_in_parent) => {
            label.to_owned()
        }
        _ => WORKTREE_MODES
            .iter()
            .find(|item| item.key == props.worktree_mode)
            .map(|item| item.label.to_owned())
            .unwrap_or_default(),
    };
    let title = if pick {
        match parent_title {
            Some(parent) if props.from_handoff => format!("New sub-session from {parent}'s handoff"),
            Some(parent) => format!("New sub-session under {parent}"),
            None => "New session".to_owned(),
        }
    } else {
        let kind = match parent_title {
            Some(_) => format!(
                " {} sub-",
                props.sub_session_kind.unwrap_or(SubSessionKind::Clean).as_str()
            ),
            None => " ".to_owned(),
        };
        format!("New {program_label}{kind}session")
    };
    let muted = TextStyle::fg(THEME.muted);
    let mut rows = wrapped(
        vec![Seg::plain(title)],
        TextStyle::fg(THEME.accent).bold(true),
        content_width,
    );
    rows.push(Vec::new());
    if pick {
        rows.extend(wrapped(vec![Seg::plain("Choose an agent")], muted, content_width));
        for (index, option) in PROGRAMS.iter().enumerate() {
            let text = format!("{} {}", option.glyph, option.label);
            rows.extend(selectable_row(
                &SelectableRowProps {
                    selected: index == props.program_index,
                    text: &text,
                    ..SelectableRowProps::default()
                },
                content_width,
            ));
        }
        if props.show_fork_option {
            rows.extend(selectable_row(
                &SelectableRowProps {
                    selected: fork_selected,
                    text: "⑂ Fork parent",
                    ..SelectableRowProps::default()
                },
                content_width,
            ));
        }
        if props.show_handoff_option {
            rows.extend(selectable_row(
                &SelectableRowProps {
                    selected: handoff_selected,
                    text: "↳ From its handoff (H)",
                    ..SelectableRowProps::default()
                },
                content_width,
            ));
        }
    } else {
        let task_title = props.task_title.filter(|title| !title.is_empty());
        if let Some(task) = task_title {
            rows.push(truncated(
                vec![Seg::plain("Task: "), Seg::new(task, TextStyle::fg(THEME.accent_soft))],
                TextStyle::default(),
                content_width,
            ));
        }
        let (name, name_color) = if props.draft_name.is_empty() {
            ("█", THEME.muted)
        } else {
            (props.draft_name, THEME.active)
        };
        rows.extend(wrapped(
            vec![Seg::plain("Name: "), Seg::new(name, TextStyle::fg(name_color))],
            TextStyle::default(),
            content_width,
        ));
        rows.extend(wrapped(
            vec![
                Seg::plain("Workspace: "),
                Seg::new(workspace_label, TextStyle::fg(THEME.accent)),
            ],
            TextStyle::default(),
            content_width,
        ));
        if let Some(base) = props.base
            && props.worktree_mode == WorktreeMode::New
            && !stays_in_parent
        {
            let mut segs = vec![
                Seg::plain("Base: "),
                Seg::new(base.label.clone(), TextStyle::fg(THEME.active)),
            ];
            if base.count > 1 {
                segs.push(Seg::new(
                    format!("  ↑↓ {}/{}", base.index + 1, base.count),
                    TextStyle::fg(THEME.muted),
                ));
            }
            rows.push(truncated(segs, TextStyle::default(), content_width));
        }
        if task_title.is_some() {
            rows.push(truncated(
                vec![Seg::plain(
                    "The task is typed into the agent's input once it's ready; edit it there and send it yourself.",
                )],
                muted,
                content_width,
            ));
        }
        if stays_in_parent {
            let text = truncate(
                &format!("{program_label} forks stay in the parent's worktree"),
                content_width,
            );
            rows.extend(wrapped(vec![Seg::plain(text)], muted, content_width));
        }
    }
    if let Some(parent) = parent_title {
        let text = format!("Parent: {}", truncate(parent, width.saturating_sub(12).max(8)));
        rows.extend(wrapped(vec![Seg::plain(text)], muted, content_width));
    }
    rows.push(Vec::new());
    let hint: Vec<HintPart> = if pick {
        vec!["enter continue".into(), "esc cancel".into()]
    } else if stays_in_parent {
        vec!["enter create".into(), "esc back".into()]
    } else {
        let mut parts: Vec<HintPart> = vec!["tab worktree".into()];
        if props.worktree_mode == WorktreeMode::New && props.base.is_some_and(|base| base.count > 1) {
            parts.push("↑↓ base".into());
        }
        parts.push("enter create".into());
        parts.push("esc back".into());
        parts
    };
    rows.push(hint_row(&hint, content_width));
    rows
}

/// TS: app.tsx CreatePane.
pub fn render_create_pane(props: &CreatePaneProps<'_>, buf: &mut Buffer, area: Rect) {
    let content = bordered(buf, area, &Border::round(THEME.border_active), 1);
    draw_rows(buf, content, &create_pane_rows(props, usize::from(area.width)));
}

/// TS: app.tsx WorktreePickerPane props (width and height are the area's).
#[derive(Clone, Copy, Debug)]
pub struct WorktreePickerProps<'a> {
    pub worktrees: &'a [&'a WorktreeInfoRecord],
    pub selected_index: usize,
    pub query: &'a str,
    pub typing: bool,
    pub total_count: usize,
}

/// TS: app.tsx WorktreePickerPane's rows inside its border.
fn worktree_picker_rows(props: &WorktreePickerProps<'_>, width: usize, height: usize) -> Vec<Row> {
    let content_width = width.saturating_sub(4).max(1);
    let muted = TextStyle::fg(THEME.muted);
    let count_label = if props.query.is_empty() {
        props.total_count.to_string()
    } else {
        format!("{}/{}", props.worktrees.len(), props.total_count)
    };
    let mut rows = wrapped(
        vec![Seg::plain("Existing worktree")],
        TextStyle::fg(THEME.accent).bold(true),
        content_width,
    );
    let search = if props.typing {
        format!("{}▏", props.query)
    } else if props.query.is_empty() {
        "/ to filter".to_owned()
    } else {
        props.query.to_owned()
    };
    let search_color = if !props.query.is_empty() || props.typing {
        THEME.active
    } else {
        THEME.muted
    };
    rows.extend(wrapped(
        vec![
            Seg::plain("Search: "),
            Seg::new(search, TextStyle::fg(search_color)),
            Seg::plain(" "),
            Seg::new(format!("({count_label})"), muted),
        ],
        TextStyle::default(),
        content_width,
    ));
    rows.push(Vec::new());
    if props.total_count == 0 {
        rows.extend(wrapped(vec![Seg::plain("No worktrees found.")], muted, content_width));
    }
    if props.total_count > 0 && props.worktrees.is_empty() {
        rows.extend(wrapped(
            vec![Seg::plain("No matching worktrees.")],
            muted,
            content_width,
        ));
    }
    if !props.worktrees.is_empty() {
        let items: Vec<MenuItem> = props
            .worktrees
            .iter()
            .map(|worktree| MenuItem {
                key: worktree.path.clone(),
                label: worktree_label(worktree, content_width.saturating_sub(2)),
                ..MenuItem::default()
            })
            .collect();
        rows.extend(menu_list(
            &MenuListProps {
                items: &items,
                selected: props.selected_index,
                rows: Some(height.saturating_sub(7).max(1)),
                label_width: None,
            },
            content_width,
        ));
    }
    rows.push(Vec::new());
    let hint: Vec<HintPart> = if props.typing {
        vec![
            "type to filter".into(),
            "enter done".into(),
            "↑↓ move".into(),
            "esc clear".into(),
        ]
    } else {
        let (search, escape) = if props.query.is_empty() {
            ("/ search", "esc back")
        } else {
            ("/ edit search", "esc clear search")
        };
        vec!["enter select".into(), search.into(), escape.into()]
    };
    rows.push(hint_row(&hint, content_width));
    rows
}

/// TS: app.tsx WorktreePickerPane.
pub fn render_worktree_picker(props: &WorktreePickerProps<'_>, buf: &mut Buffer, area: Rect) {
    let content = bordered(buf, area, &Border::round(THEME.border_active), 1);
    let rows = worktree_picker_rows(props, usize::from(area.width), usize::from(area.height));
    draw_rows(buf, content, &rows);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::layout::render_app;
    use crate::app::state::{AppProps, Review};
    use crate::app::test_support::{app, session};
    use crate::primitives::row_text;
    use deckhand_client::live::{LiveClient, LiveClientHandlers};
    use deckhand_core::key::Key;
    use deckhand_protocol::data::{BranchList, SessionStatus};
    use deckhand_protocol::records::SessionFilter;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use serde_json::{Value, json};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::UnixStream;
    use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
    use tokio::sync::mpsc::UnboundedReceiver;

    type Updates = UnboundedReceiver<crate::app::event_loop::Update<App>>;

    fn run(test: impl std::future::Future<Output = ()>) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap_or_else(|error| panic!("{error}"));
        tokio::task::LocalSet::new().block_on(&runtime, test);
    }

    /// The daemon end of the app's connection: reads requests, answers them.
    struct Daemon {
        lines: BufReader<OwnedReadHalf>,
        writer: OwnedWriteHalf,
    }

    impl Daemon {
        async fn next(&mut self) -> Value {
            let mut line = String::new();
            self.lines
                .read_line(&mut line)
                .await
                .unwrap_or_else(|error| panic!("{error}"));
            serde_json::from_str(&line).unwrap_or_else(|error| panic!("{line}: {error}"))
        }

        async fn respond(&mut self, request: &Value, ok: bool, body: Value) {
            let message = if ok {
                json!({"type": "response", "requestId": request["requestId"], "ok": true, "data": body})
            } else {
                json!({"type": "response", "requestId": request["requestId"], "ok": false, "error": body})
            };
            self.writer
                .write_all(format!("{message}\n").as_bytes())
                .await
                .unwrap_or_else(|error| panic!("{error}"));
        }
    }

    /// An app for `/repo` connected to a fake daemon.
    fn connected() -> (App, Updates, Daemon) {
        let (mut app, rx) = app(AppProps::default());
        let (socket, peer) = UnixStream::pair().unwrap_or_else(|error| panic!("{error}"));
        app.connection.client = Some(Rc::new(LiveClient::new(socket, LiveClientHandlers::default())));
        let (read, writer) = peer.into_split();
        let daemon = Daemon {
            lines: BufReader::new(read),
            writer,
        };
        (app, rx, daemon)
    }

    /// Runs the next update sent to the app (a settled reply).
    async fn apply(app: &mut App, rx: &mut Updates) {
        let update = rx.recv().await.unwrap_or_else(|| panic!("no update"));
        update(app);
    }

    fn project(needs_review: bool, config: Value, effective: Value) -> Value {
        json!({
            "root": "/repo", "trustRoot": "/repo", "config": config, "exists": true, "fingerprint": "fp1",
            "trusted": !needs_review, "needsReview": needs_review, "effective": effective,
        })
    }

    fn project_info(config: Value, creation_hook: Option<&str>) -> ProjectInfo {
        let mut value = project(true, config, json!({}));
        if let Some(file) = creation_hook {
            value["creationHook"] = json!({"file": file, "content": "#!/bin/sh", "fingerprint": "h"});
        }
        serde_json::from_value(value).unwrap_or_else(|error| panic!("{error}"))
    }

    fn key(key: Key) -> Input {
        Input::key("", key)
    }

    fn escape() -> Input {
        key(Key {
            escape: true,
            meta: true,
            ..Key::NONE
        })
    }

    fn enter() -> Input {
        key(Key {
            return_: true,
            ..Key::NONE
        })
    }

    fn tab() -> Input {
        key(Key { tab: true, ..Key::NONE })
    }

    fn up() -> Input {
        key(Key {
            up_arrow: true,
            ..Key::NONE
        })
    }

    fn down() -> Input {
        key(Key {
            down_arrow: true,
            ..Key::NONE
        })
    }

    fn backspace() -> Input {
        key(Key {
            backspace: true,
            ..Key::NONE
        })
    }

    fn worktree(path: &str, branch: &str, is_main: bool) -> WorktreeInfoRecord {
        WorktreeInfoRecord {
            path: path.to_owned(),
            branch: branch.to_owned(),
            head: "abc".to_owned(),
            is_main,
        }
    }

    fn branches(names: &[&str]) -> BranchList {
        serde_json::from_value(json!({"branches": names, "current": "main", "branchFrom": "current"}))
            .unwrap_or_else(|error| panic!("{error}"))
    }

    fn texts(rows: &[Row]) -> Vec<String> {
        rows.iter().map(|row| row_text(row).trim_end().to_owned()).collect()
    }

    #[test]
    fn untrusted_parts_list_what_a_new_worktree_would_run() {
        assert!(untrusted_creation_parts(&project_info(json!({"devCommand": "npm run dev"}), None)).is_empty());
        let parts = untrusted_creation_parts(&project_info(
            json!({"setupCommand": "npm ci", "worktree": {"files": {"a": "b"}, "location": "../wt", "branchName": "x/{name}"}}),
            Some(".deckhand/create-worktree"),
        ));
        assert_eq!(
            parts,
            [
                "setupCommand (npm ci)",
                "creation hook (.deckhand/create-worktree)",
                "worktree settings (location, branchName, files)"
            ]
        );
        // An empty setup command runs nothing; `hook` alone is not a creation setting.
        let parts = untrusted_creation_parts(&project_info(
            json!({"setupCommand": "", "worktree": {"hook": false}}),
            None,
        ));
        assert!(parts.is_empty());
    }

    #[test]
    fn worktrees_filter_on_every_term_and_label_with_main_first() {
        let list = [
            worktree("/repo", "main", true),
            worktree("/wt/feature-login", "feature/login", false),
            worktree("/wt/detached", "", false),
        ];
        assert_eq!(filtered_worktrees(&list, "  ").len(), 3);
        let found: Vec<&str> = filtered_worktrees(&list, "FEATURE  login")
            .iter()
            .map(|worktree| worktree.path.as_str())
            .collect();
        assert_eq!(found, ["/wt/feature-login"]);
        assert!(filtered_worktrees(&list, "feature nope").is_empty());
        assert_eq!(filtered_worktrees(&list, "WT").len(), 2);
        assert_eq!(worktree_label(&list[0], 40), "main · main  /repo");
        assert_eq!(worktree_label(&list[2], 40), "(detached)  /wt/detached");
        assert_eq!(worktree_label(&list[1], 10), "feature/l…");
        assert_eq!(worktree_label(&list[1], 0), "");
    }

    #[test]
    fn the_agent_picker_offers_fork_and_handoff_for_a_parent() {
        let (mut app, _rx) = app(AppProps::default());
        let mut parent = session("p1", "beta", SessionStatus::Running);
        parent.handoff_path = Some("/repo/handoff.md".to_owned());
        app.data.repo_sessions = vec![parent];
        app.create.parent_id = Some("p1".to_owned());
        app.create.sub_session_kind = Some(SubSessionKind::Clean);
        app.set_mode(Mode::PickProgram);
        // Three agents, ⑂ fork and ↳ handoff: five options, wrapping.
        handle_pick_program(&mut app, &up());
        assert_eq!(app.create.program_index, 4);
        handle_pick_program(&mut app, &Input::text("j"));
        assert_eq!(app.create.program_index, 0);
        handle_pick_program(&mut app, &Input::text("k"));
        handle_pick_program(&mut app, &enter());
        // ↳ from its handoff: a clean child of the parent's agent, without a worktree; the agent is picked next.
        assert_eq!(app.mode, Mode::PickProgram);
        assert_eq!(app.create.handoff_from_id.as_deref(), Some("p1"));
        assert_eq!(app.create.program_index, 0);
        assert_eq!(app.create.worktree_mode, WorktreeMode::None);
        // Now only the agents: no fork, no handoff row.
        handle_pick_program(&mut app, &up());
        assert_eq!(app.create.program_index, 2);
        // Esc goes back to the sub-session choices first, then cancels.
        handle_pick_program(&mut app, &escape());
        assert_eq!(app.create.handoff_from_id, None);
        assert_eq!(app.create.program_index, 0);
        assert_eq!(app.mode, Mode::PickProgram);
        handle_pick_program(&mut app, &down());
        handle_pick_program(&mut app, &down());
        handle_pick_program(&mut app, &down());
        handle_pick_program(&mut app, &enter());
        assert_eq!(app.mode, Mode::EnterName);
        assert_eq!(app.create.sub_session_kind, Some(SubSessionKind::Forked));
        assert_eq!(app.create.program_index, 0);
        app.set_mode(Mode::PickProgram);
        handle_pick_program(&mut app, &escape());
        assert_eq!(app.mode, Mode::Browse);
        assert_eq!(app.create.parent_id, None);
        assert_eq!(app.create.sub_session_kind, None);
    }

    #[test]
    fn a_codex_fork_stays_in_the_parent_directory() {
        let (mut app, _rx) = app(AppProps::default());
        let mut parent = session("p1", "beta", SessionStatus::Running);
        parent.program = ProgramKey::Codex;
        app.data.repo_sessions = vec![parent];
        app.create.parent_id = Some("p1".to_owned());
        app.create.worktree_mode = WorktreeMode::New;
        app.create.program_index = 3;
        app.set_mode(Mode::PickProgram);
        handle_pick_program(&mut app, &enter());
        assert_eq!(app.mode, Mode::EnterName);
        assert_eq!(app.create.program_index, 2);
        assert_eq!(app.create.worktree_mode, WorktreeMode::None);
        // Tab does nothing: the workspace is the parent's.
        handle_enter_name(&mut app, &tab());
        assert_eq!(app.create.worktree_mode, WorktreeMode::None);
        let base = BaseShown {
            label: "main".to_owned(),
            index: 0,
            count: 3,
        };
        let props = CreatePaneProps {
            mode: CreateStep::EnterName,
            program_index: 2,
            draft_name: "",
            worktree_mode: WorktreeMode::New,
            parent_title: Some("beta"),
            parent_workspace_label: Some("parent dir: /repo"),
            sub_session_kind: Some(SubSessionKind::Forked),
            show_fork_option: false,
            show_handoff_option: false,
            from_handoff: false,
            task_title: None,
            base: Some(&base),
        };
        assert_eq!(
            texts(&create_pane_rows(&props, 60)),
            [
                "New Codex forked sub-session",
                "",
                "Name: █",
                "Workspace: parent dir: /repo",
                "Codex forks stay in the parent's worktree",
                "Parent: beta",
                "",
                "enter create • esc back"
            ]
        );
    }

    #[test]
    fn esc_from_a_task_start_returns_to_the_board() {
        let (mut app, _rx) = app(AppProps::default());
        app.create.task_start = Some(TaskStart {
            id: "t1".to_owned(),
            title: "Write docs".to_owned(),
        });
        app.set_mode(Mode::PickProgram);
        handle_pick_program(&mut app, &escape());
        assert_eq!(app.mode, Mode::Tasks);
        assert_eq!(app.create.task_start, None);
    }

    #[test]
    fn the_name_form_types_cycles_workspaces_and_bases() {
        let (mut app, _rx) = app(AppProps::default());
        app.set_mode(Mode::EnterName);
        handle_enter_name(&mut app, &Input::text("fix bug #1!"));
        handle_enter_name(&mut app, &Input::text("[A"));
        let ctrl_x = Input::key(
            "x",
            Key {
                ctrl: true,
                ..Key::NONE
            },
        );
        handle_enter_name(&mut app, &ctrl_x);
        assert_eq!(app.create.draft_name, "fix bug #1");
        handle_enter_name(&mut app, &backspace());
        assert_eq!(app.create.draft_name, "fix bug #");
        handle_enter_name(&mut app, &tab());
        assert_eq!(app.create.worktree_mode, WorktreeMode::New);
        // ↑↓ walk the bases (once listed), wrapping.
        handle_enter_name(&mut app, &down());
        assert_eq!(app.create.base_index, 0);
        app.create.branch_list = Some(branches(&["main", "dev"]));
        handle_enter_name(&mut app, &up());
        assert_eq!(app.create.base_index, 2);
        handle_enter_name(&mut app, &down());
        assert_eq!(app.create.base_index, 0);
        handle_enter_name(&mut app, &tab());
        assert_eq!(app.create.worktree_mode, WorktreeMode::Existing);
        handle_enter_name(&mut app, &down());
        assert_eq!(app.create.base_index, 0);
        handle_enter_name(&mut app, &tab());
        assert_eq!(app.create.worktree_mode, WorktreeMode::None);
        handle_enter_name(&mut app, &escape());
        assert_eq!(app.mode, Mode::PickProgram);
    }

    #[test]
    fn creating_needs_a_title_and_a_connection() {
        let (mut app, _rx) = app(AppProps::default());
        app.set_mode(Mode::EnterName);
        app.create.draft_name = "   ".to_owned();
        for mode in [WorktreeMode::None, WorktreeMode::New, WorktreeMode::Existing] {
            app.create.worktree_mode = mode;
            app.set_error(None);
            handle_enter_name(&mut app, &enter());
            assert_eq!(app.messages.error.as_deref(), Some("title cannot be empty"));
        }
        app.create.draft_name = "alpha".to_owned();
        for mode in [WorktreeMode::None, WorktreeMode::Existing] {
            app.create.worktree_mode = mode;
            app.set_error(None);
            handle_enter_name(&mut app, &enter());
            assert_eq!(app.messages.error.as_deref(), Some("still connecting to daemon"));
        }
        // A new worktree reviews first, which needs the daemon: without one nothing happens.
        app.create.worktree_mode = WorktreeMode::New;
        app.set_error(None);
        handle_enter_name(&mut app, &enter());
        assert_eq!(app.messages.error, None);
        assert_eq!(app.mode, Mode::EnterName);
        assert!(!app.messages.busy);
    }

    #[test]
    fn the_worktree_picker_searches_and_selects() {
        let (mut app, _rx) = app(AppProps::default());
        app.create.worktrees = vec![
            worktree("/repo", "main", true),
            worktree("/wt/a", "feature-a", false),
            worktree("/wt/b", "feature-b", false),
        ];
        app.set_mode(Mode::PickWorktree);
        handle_pick_worktree(&mut app, &Input::text("j"));
        handle_pick_worktree(&mut app, &down());
        handle_pick_worktree(&mut app, &down());
        assert_eq!(app.create.worktree_index, 2);
        handle_pick_worktree(&mut app, &Input::text("k"));
        assert_eq!(app.create.worktree_index, 1);
        handle_pick_worktree(&mut app, &Input::text("/"));
        assert!(app.create.worktree_typing);
        // While typing, j and k are text.
        handle_pick_worktree(&mut app, &Input::text("feature-b"));
        handle_pick_worktree(&mut app, &Input::text("j"));
        assert_eq!(app.create.worktree_query, "feature-bj");
        assert_eq!(app.create.worktree_index, 0);
        handle_pick_worktree(&mut app, &backspace());
        handle_pick_worktree(&mut app, &tab());
        assert_eq!(app.create.worktree_query, "feature-b");
        handle_pick_worktree(&mut app, &enter());
        assert!(!app.create.worktree_typing);
        assert_eq!(
            filtered_worktrees(&app.create.worktrees, &app.create.worktree_query).len(),
            1
        );
        handle_pick_worktree(&mut app, &down());
        assert_eq!(app.create.worktree_index, 0);
        // Esc clears the search first, then goes back to the form.
        handle_pick_worktree(&mut app, &escape());
        assert_eq!(app.create.worktree_query, "");
        assert_eq!(app.mode, Mode::PickWorktree);
        handle_pick_worktree(&mut app, &Input::text("/"));
        handle_pick_worktree(&mut app, &Input::text("zz"));
        handle_pick_worktree(&mut app, &escape());
        assert!(!app.create.worktree_typing);
        assert_eq!(app.create.worktree_query, "");
        handle_pick_worktree(&mut app, &escape());
        assert_eq!(app.mode, Mode::EnterName);
        // Enter with no match does nothing.
        app.set_mode(Mode::PickWorktree);
        app.create.worktree_query = "nothing".to_owned();
        app.create.draft_name = "x".to_owned();
        handle_pick_worktree(&mut app, &enter());
        assert_eq!(app.messages.error, None);
        app.create.worktree_query.clear();
        handle_pick_worktree(&mut app, &enter());
        assert_eq!(app.messages.error.as_deref(), Some("still connecting to daemon"));
    }

    #[test]
    fn n_opens_the_form_with_the_repository_defaults() {
        run(async {
            let (mut app, mut rx, mut daemon) = connected();
            app.browse.session_query = "zz".to_owned();
            app.browse.session_filter = SessionFilter::All;
            app.create.draft_name = "stale".to_owned();
            open_new_session(&mut app, false);
            assert!(app.messages.busy);
            let request = daemon.next().await;
            assert_eq!(request["type"], "project-info");
            assert_eq!(request["cwd"], "/repo");
            let effective = json!({"defaultAgent": "codex", "defaultWorkspace": "new"});
            daemon
                .respond(&request, true, project(true, json!({}), effective))
                .await;
            apply(&mut app, &mut rx).await;
            assert!(!app.messages.busy);
            // Untrusted or not, the defaults only preselect: no review here.
            assert_eq!(app.mode, Mode::PickProgram);
            assert_eq!(app.create.program_index, 2);
            assert_eq!(app.create.worktree_mode, WorktreeMode::New);
            assert_eq!(app.create.draft_name, "");
            assert_eq!(app.create.parent_id, None);
            assert_eq!(app.browse.session_query, "");
            assert_eq!(app.browse.session_filter, SessionFilter::Active);
        });
    }

    #[test]
    fn shift_n_keeps_the_parents_agent_and_survives_a_failed_lookup() {
        run(async {
            let (mut app, mut rx, mut daemon) = connected();
            let mut parent = session("p1", "beta", SessionStatus::Running);
            parent.program = ProgramKey::Pi;
            parent.cwd = "/repo/wt".to_owned();
            app.data.repo_sessions = vec![parent];
            app.browse.selected_id = Some("p1".to_owned());
            open_new_session(&mut app, true);
            let request = daemon.next().await;
            assert_eq!(request["cwd"], "/repo/wt");
            daemon.respond(&request, false, json!("no config")).await;
            apply(&mut app, &mut rx).await;
            assert_eq!(app.messages.error.as_deref(), Some("no config"));
            assert_eq!(app.mode, Mode::PickProgram);
            assert_eq!(app.create.program_index, 1);
            assert_eq!(app.create.parent_id.as_deref(), Some("p1"));
            assert_eq!(app.create.sub_session_kind, Some(SubSessionKind::Clean));
            assert_eq!(app.create.worktree_mode, WorktreeMode::None);
        });
    }

    #[test]
    fn a_task_starts_the_form_in_a_new_worktree() {
        run(async {
            let (mut app, mut rx, mut daemon) = connected();
            app.create.parent_id = Some("old".to_owned());
            let task: Task = serde_json::from_value(json!({
                "id": "t1", "title": "Write the docs: README & guide!", "done": false, "body": "", "meta": {}
            }))
            .unwrap_or_else(|error| panic!("{error}"));
            start_from_task(&mut app, &task);
            let request = daemon.next().await;
            daemon
                .respond(&request, true, project(false, json!({}), json!({"defaultAgent": "pi"})))
                .await;
            apply(&mut app, &mut rx).await;
            assert_eq!(app.mode, Mode::PickProgram);
            assert_eq!(app.create.program_index, 1);
            assert_eq!(app.create.draft_name, "Write the docs: README guide");
            assert_eq!(app.create.worktree_mode, WorktreeMode::New);
            assert_eq!(app.create.parent_id, None);
            assert_eq!(
                app.create.task_start,
                Some(TaskStart {
                    id: "t1".to_owned(),
                    title: "Write the docs: README & guide!".to_owned()
                })
            );
            // The new worktree's base branches load once the form shows it.
            app.set_mode(Mode::EnterName);
            app.effects.branches.generation = 7;
            load_branches(&mut app);
            let request = daemon.next().await;
            assert_eq!(request["type"], "list-branches");
            let list = json!({"branches": ["main", "dev"], "branchFrom": "default", "defaultBranch": "main"});
            daemon.respond(&request, true, list).await;
            apply(&mut app, &mut rx).await;
            assert_eq!(app.create.branch_list.as_ref().map(|list| list.branches.len()), Some(2));
            app.create.base_index = 2;
            // Creating links the task and starts from the picked base.
            handle_enter_name(&mut app, &enter());
            let request = daemon.next().await;
            assert_eq!(request["type"], "project-info");
            daemon
                .respond(&request, true, project(false, json!({}), json!({})))
                .await;
            apply(&mut app, &mut rx).await;
            let request = daemon.next().await;
            assert_eq!(request["type"], "create");
            let input = &request["input"];
            assert_eq!(input["title"], "Write the docs: README guide");
            assert_eq!(input["program"], "pi");
            assert_eq!(input["worktreeMode"], "new");
            assert_eq!(input["baseBranch"], "dev");
            assert_eq!(input["taskId"], "t1");
            assert!(input.get("subSessionKind").is_none());
            assert!(input.get("projectFingerprint").is_none());
        });
    }

    #[test]
    fn a_stale_branch_listing_is_dropped() {
        run(async {
            let (mut app, mut rx, mut daemon) = connected();
            load_branches(&mut app);
            let request = daemon.next().await;
            app.effects.branches.generation += 1;
            daemon
                .respond(&request, true, json!({"branches": [], "branchFrom": "current"}))
                .await;
            apply(&mut app, &mut rx).await;
            assert_eq!(app.create.branch_list, None);
        });
    }

    #[test]
    fn a_new_worktree_reviews_untrusted_creation_settings_first() {
        run(async {
            let (mut app, mut rx, mut daemon) = connected();
            app.create.draft_name = " alpha ".to_owned();
            app.create.worktree_mode = WorktreeMode::New;
            app.set_mode(Mode::EnterName);
            handle_enter_name(&mut app, &enter());
            let request = daemon.next().await;
            let config = json!({"setupCommand": "npm ci", "worktree": {"symlink": ["node_modules"]}});
            daemon.respond(&request, true, project(true, config, json!({}))).await;
            apply(&mut app, &mut rx).await;
            assert_eq!(app.mode, Mode::ReviewProject);
            let review: Review = app.review.review.clone().unwrap_or_else(|| panic!("no review"));
            assert_eq!(
                review.purpose.as_deref(),
                Some(
                    "About to create a new worktree using this repository's setupCommand (npm ci), worktree settings \
                     (symlink)."
                )
            );
            let labels = review.labels.clone().unwrap_or_default();
            assert_eq!(labels.enter, Some(hint_short("enter trust & create", "enter trust")));
            assert_eq!(
                labels.skip.map(|skip| skip.text),
                Some("s create without the repo's settings (global only)".to_owned())
            );
            // Esc returns to the form.
            if let Some(back) = review.back.as_ref() {
                back(&mut app);
            }
            assert_eq!(app.mode, Mode::EnterName);
            // s creates without the repository's settings: the reviewed fingerprint goes along.
            let ReviewSkip::Run(skip) = review.skip.clone() else {
                panic!("skip should create");
            };
            skip(&mut app, review.project.clone());
            assert!(app.messages.busy);
            let request = daemon.next().await;
            let input = &request["input"];
            assert_eq!(input["title"], "alpha");
            assert_eq!(input["projectFingerprint"], "fp1");
            let layout = app.layout();
            assert_eq!(input["cols"], layout.preview_cols);
            assert_eq!(input["rows"], layout.preview_rows);
            assert!(input.get("baseBranch").is_none());
            daemon.respond(&request, false, json!("worktree already exists")).await;
            apply(&mut app, &mut rx).await;
            assert!(!app.messages.busy);
            assert_eq!(app.messages.error.as_deref(), Some("worktree already exists"));
            assert_eq!(app.create.draft_name, " alpha ");
        });
    }

    #[test]
    fn creating_a_sub_session_in_an_existing_worktree() {
        run(async {
            let (mut app, mut rx, mut daemon) = connected();
            let mut parent = session("p1", "beta", SessionStatus::Running);
            parent.cwd = "/repo/wt".to_owned();
            app.data.repo_sessions = vec![parent];
            app.create.parent_id = Some("p1".to_owned());
            app.create.sub_session_kind = Some(SubSessionKind::Forked);
            app.create.draft_name = "gamma".to_owned();
            app.create.worktree_mode = WorktreeMode::Existing;
            app.create.task_start = Some(TaskStart {
                id: "t9".to_owned(),
                title: "ignored for children".to_owned(),
            });
            app.set_mode(Mode::EnterName);
            handle_enter_name(&mut app, &enter());
            assert!(app.messages.busy);
            let request = daemon.next().await;
            assert_eq!(request["type"], "list-worktrees");
            assert_eq!(request["cwd"], "/repo/wt");
            let listed = json!([{"path": "/repo", "branch": "main", "head": "a", "isMain": true},
                                {"path": "/repo/wt", "branch": "feat", "head": "b", "isMain": false}]);
            daemon.respond(&request, true, listed).await;
            apply(&mut app, &mut rx).await;
            assert!(!app.messages.busy);
            assert_eq!(app.mode, Mode::PickWorktree);
            assert_eq!(app.create.worktrees.len(), 2);
            handle_pick_worktree(&mut app, &down());
            handle_pick_worktree(&mut app, &enter());
            let request = daemon.next().await;
            let input = &request["input"];
            assert_eq!(input["cwd"], "/repo/wt");
            assert_eq!(input["existingWorktreePath"], "/repo/wt");
            assert_eq!(input["parentSessionId"], "p1");
            assert_eq!(input["subSessionKind"], "forked");
            assert!(input.get("taskId").is_none());
            let created = serde_json::to_value(session("c1", "gamma", SessionStatus::Starting))
                .unwrap_or_else(|error| panic!("{error}"));
            daemon.respond(&request, true, created).await;
            apply(&mut app, &mut rx).await;
            assert_eq!(app.mode, Mode::Browse);
            assert_eq!(app.browse.selected_id.as_deref(), Some("c1"));
            assert!(app.data.repo_sessions.iter().any(|session| session.id == "c1"));
            assert_eq!(app.create.draft_name, "");
            assert_eq!(app.create.parent_id, None);
            assert_eq!(app.create.sub_session_kind, None);
            assert_eq!(app.create.task_start, None);
            assert_eq!(app.create.worktree_mode, WorktreeMode::None);
        });
    }

    #[test]
    fn a_failed_worktree_listing_shows_the_error() {
        run(async {
            let (mut app, mut rx, mut daemon) = connected();
            app.create.draft_name = "x".to_owned();
            app.create.worktree_mode = WorktreeMode::Existing;
            app.set_mode(Mode::EnterName);
            handle_enter_name(&mut app, &enter());
            let request = daemon.next().await;
            daemon.respond(&request, false, json!("not a git repository")).await;
            apply(&mut app, &mut rx).await;
            assert_eq!(app.messages.error.as_deref(), Some("not a git repository"));
            assert_eq!(app.mode, Mode::EnterName);
            assert!(!app.messages.busy);
        });
    }

    #[test]
    fn the_task_form_shows_the_task_and_the_base() {
        let base = BaseShown {
            label: "main (default branch)".to_owned(),
            index: 0,
            count: 1,
        };
        let props = CreatePaneProps {
            mode: CreateStep::EnterName,
            program_index: 0,
            draft_name: "Write docs",
            worktree_mode: WorktreeMode::New,
            parent_title: None,
            parent_workspace_label: None,
            sub_session_kind: None,
            show_fork_option: false,
            show_handoff_option: false,
            from_handoff: false,
            task_title: Some("Write docs"),
            base: Some(&base),
        };
        assert_eq!(
            texts(&create_pane_rows(&props, 60)),
            [
                "New Claude session",
                "",
                "Task: Write docs",
                "Name: Write docs",
                "Workspace: new worktree",
                "Base: main (default branch)",
                "The task is typed into the agent's input once it's read…",
                "",
                "tab worktree • enter create • esc back"
            ]
        );
        let handoff = CreatePaneProps {
            mode: CreateStep::PickProgram,
            parent_title: Some("beta"),
            from_handoff: true,
            task_title: None,
            ..props
        };
        assert_eq!(
            texts(&create_pane_rows(&handoff, 60))[0],
            "New sub-session from beta's handoff"
        );
        let options = CreatePaneProps {
            show_fork_option: true,
            show_handoff_option: true,
            from_handoff: false,
            program_index: 4,
            ..handoff
        };
        let rows = texts(&create_pane_rows(&options, 60));
        assert_eq!(rows[0], "New sub-session under beta");
        assert_eq!(rows[6], "  ⑂ Fork parent");
        assert_eq!(rows[7], "❯ ↳ From its handoff (H)");
        assert_eq!(rows[8], "Parent: beta");
    }

    #[test]
    fn the_worktree_picker_reports_empty_and_unmatched_lists() {
        let rows = |worktrees: &[&WorktreeInfoRecord], query: &str, typing: bool, total: usize| {
            let props = WorktreePickerProps {
                worktrees,
                selected_index: 0,
                query,
                typing,
                total_count: total,
            };
            texts(&worktree_picker_rows(&props, 60, 20))
        };
        assert_eq!(
            rows(&[], "", false, 0),
            [
                "Existing worktree",
                "Search: / to filter (0)",
                "",
                "No worktrees found.",
                "",
                "enter select • / search • esc back"
            ]
        );
        assert_eq!(
            rows(&[], "zz", true, 3),
            [
                "Existing worktree",
                "Search: zz▏ (0/3)",
                "",
                "No matching worktrees.",
                "",
                "type to filter • enter done • ↑↓ move • esc clear"
            ]
        );
        let one = worktree("/wt/a", "feature-a", false);
        assert_eq!(
            rows(&[&one], "feat", false, 3),
            [
                "Existing worktree",
                "Search: feat (1/3)",
                "",
                "❯ feature-a  /wt/a",
                "",
                "enter select • / edit search • esc clear search"
            ]
        );
    }

    #[test]
    fn keymaps_name_documented_keys() {
        for keymap in [PICK_PROGRAM_KEYMAP, ENTER_NAME_KEYMAP, PICK_WORKTREE_KEYMAP] {
            assert!(crate::keymap::undocumented_in_guide(keymap).is_empty());
            assert!(crate::keymap::unknown_topics(keymap).is_empty());
        }
        assert!(PICK_WORKTREE_KEYMAP[2].matches(&Input::text("/")));
    }

    /// Renders the whole app (130×36, repo `<TMP>/demo`) after `setup` and compares the right pane with the snapshot
    /// `fixtures/ui/panes/<name>.txt`.
    fn assert_right_pane(name: &str, setup: impl FnOnce(&mut App)) {
        let (mut app, _rx) = app(AppProps::default());
        app.props.repo_root = "<TMP>/demo".to_owned();
        app.props.cwd = "<TMP>/demo".to_owned();
        app.data.sessions_loaded = true;
        // This snapshot covers a 98-column pane, with the sidebar explicitly sized to leave that space.
        app.browse.sidebar_percent = Some(24.0);
        setup(&mut app);
        let size = app.browse.terminal_size;
        let mut terminal =
            Terminal::new(TestBackend::new(size.cols, size.rows)).unwrap_or_else(|error| panic!("{error}"));
        terminal
            .draw(|frame| render_app(&mut app, frame))
            .unwrap_or_else(|error| panic!("{error}"));
        let buffer = terminal.backend().buffer();
        let layout = app.layout();
        let left = layout.sidebar_width + 1;
        let rows: Vec<String> = (2..2 + layout.content_height)
            .map(|y| {
                let mut line = String::new();
                let mut x = left;
                while x < buffer.area.width {
                    let symbol = buffer[(x, y)].symbol();
                    line.push_str(symbol);
                    x += u16::try_from(crate::primitives::string_width(symbol).max(1)).unwrap_or(1);
                }
                line.trim_end().to_owned()
            })
            .collect();
        deckhand_testkit::assert_snapshot(format!("fixtures/ui/panes/{name}.txt"), &rows.join("\n"));
    }

    #[test]
    fn renders_the_new_session_form_like_the_snapshots() {
        assert_right_pane("23-create-pick-agent", |app| app.set_mode(Mode::PickProgram));
        let named = |mode: WorktreeMode| {
            move |app: &mut App| {
                app.set_mode(Mode::EnterName);
                app.create.draft_name = "gamma".to_owned();
                app.create.worktree_mode = mode;
                app.create.branch_list = Some(branches(&["main"]));
            }
        };
        assert_right_pane("24-create-name", named(WorktreeMode::None));
        assert_right_pane("25-create-worktree-new", named(WorktreeMode::New));
        assert_right_pane("26-create-worktree-existing", named(WorktreeMode::Existing));
        assert_right_pane("27-pick-worktree", |app| {
            app.set_mode(Mode::PickWorktree);
            app.create.worktrees = vec![worktree("<TMP>/demo", "main", true)];
        });
        assert_right_pane("30-create-subsession-pick", |app| {
            let mut alpha = session("s1", "alpha", SessionStatus::Running);
            let mut beta = session("s2", "beta", SessionStatus::Running);
            for record in [&mut alpha, &mut beta] {
                record.cwd = "<TMP>/demo".to_owned();
                record.repo_root = "<TMP>/demo".to_owned();
            }
            app.data.repo_sessions = vec![alpha, beta];
            app.browse.selected_id = Some("s2".to_owned());
            app.create.parent_id = Some("s2".to_owned());
            app.create.sub_session_kind = Some(SubSessionKind::Clean);
            app.set_mode(Mode::PickProgram);
        });
    }
}
