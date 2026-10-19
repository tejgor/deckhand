//! Port of app.tsx's new-session flow (n / N, n on a task): pick-program (agent, fork, from handoff), enter-name
//! (name, workspace, base branch) and pick-worktree (an existing worktree, searchable), the CreatePane and
//! WorktreePickerPane, and creating the session (reviewing an untrusted repository's creation parts first).

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use deckhand_core::key::Input;
use deckhand_protocol::data::{ProjectInfo, SubSessionKind, Task, WorktreeInfoRecord, WorktreeMode};

use crate::app::state::App;
use crate::keymap::Keymap;
use crate::primitives::Row;

/// pick-program keys.
pub static PICK_PROGRAM_KEYMAP: Keymap = &[];
/// enter-name keys.
pub static ENTER_NAME_KEYMAP: Keymap = &[];
/// pick-worktree keys.
pub static PICK_WORKTREE_KEYMAP: Keymap = &[];

/// TS: n / N in browse: the form (children keep their parent's agent and directory; the repository's defaults
/// preselect the picker).
pub fn open_new_session(app: &mut App, sub_session: bool) {
    let _ = (app, sub_session);
    todo!("A3")
}

/// TS `startFromTask` (n on a backlog task): the form filled in from the task, in a new worktree by default.
pub fn start_from_task(app: &mut App, task: &Task) {
    let _ = (app, task);
    todo!("A3")
}

/// TS: the pick-program branch of the useTerminalInput handler.
pub fn handle_pick_program(app: &mut App, input: &Input) {
    let _ = (app, input);
    todo!("A3")
}

/// TS: the enter-name branch.
pub fn handle_enter_name(app: &mut App, input: &Input) {
    let _ = (app, input);
    todo!("A3")
}

/// TS: the pick-worktree branch.
pub fn handle_pick_worktree(app: &mut App, input: &Input) {
    let _ = (app, input);
    todo!("A3")
}

/// TS `untrustedCreationParts`: what creating a new worktree would take from an untrusted repository config.
pub fn untrusted_creation_parts(project: &ProjectInfo) -> Vec<String> {
    let _ = project;
    todo!("A3")
}

/// TS `confirmCreate`: create, reviewing the untrusted parts a new worktree would use first.
pub fn confirm_create(app: &mut App) {
    let _ = app;
    todo!("A3")
}

/// TS `submitCreate` (`project_fingerprint`: the repository config reviewed and skipped).
pub fn submit_create(app: &mut App, existing_worktree_path: Option<String>, project_fingerprint: Option<String>) {
    let _ = (app, existing_worktree_path, project_fingerprint);
    todo!("A3")
}

/// TS: the base-branches effect: loaded once the form shows a new worktree.
pub fn load_branches(app: &mut App) {
    let _ = app;
    todo!("A3")
}

/// TS `filteredWorktrees`: every search term in the branch or path (case-insensitive).
pub fn filtered_worktrees<'a>(worktrees: &'a [WorktreeInfoRecord], query: &str) -> Vec<&'a WorktreeInfoRecord> {
    let _ = (worktrees, query);
    todo!("A3")
}

/// TS `worktreeLabel` (`main · branch  path`).
pub fn worktree_label(worktree: &WorktreeInfoRecord, width: usize) -> String {
    let _ = (worktree, width);
    todo!("A3")
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

/// TS: app.tsx CreatePane's rows inside its border.
pub fn create_pane_rows(props: &CreatePaneProps<'_>, width: usize) -> Vec<Row> {
    let _ = (props, width);
    todo!("A3")
}

/// TS: app.tsx CreatePane.
pub fn render_create_pane(props: &CreatePaneProps<'_>, buf: &mut Buffer, area: Rect) {
    let _ = (props, buf, area);
    todo!("A3")
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

/// TS: app.tsx WorktreePickerPane.
pub fn render_worktree_picker(props: &WorktreePickerProps<'_>, buf: &mut Buffer, area: Rect) {
    let _ = (props, buf, area);
    todo!("A3")
}
