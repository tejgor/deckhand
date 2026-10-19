//! Port of worktreesFlow.tsx: W, the worktree manager. Every worktree of the repository (Git's list plus
//! Deckhand's records), grouped by what to do with it, with its sessions, merge state and what deleting it would
//! lose. x deletes one (or, on the ready group's heading, all of them) after a confirmation that names the sessions
//! it stops; data loss still needs a typed DELETE. The daemon re-checks everything at deletion time.

use std::rc::Rc;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use deckhand_client::live::LiveClient;
use deckhand_core::key::Input;
use deckhand_core::worktrees_model::WorktreeRow;
use deckhand_protocol::data::{SessionRecord, WorktreeCleanupInspection, WorktreeOverview, WorktreeOverviewEntry};

use crate::app::state::App;
use crate::flow::{Flow, FlowCx};
use crate::keymap::Keymap;
use crate::primitives::Row;

/// TS: worktreesFlow.tsx DETAIL_ROWS.
pub const DETAIL_ROWS: usize = 5;

/// TS: worktreesFlow.tsx ConfirmKind.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ConfirmKind {
    Delete,
    DeleteBranch,
    Prune,
    Forget,
    Cancel,
}

/// TS: worktreesFlow.tsx ConfirmOption.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfirmOption {
    pub kind: ConfirmKind,
    pub label: String,
}

/// TS: worktreesFlow.tsx DeleteConfirm `checks`: the daemon's fresh check of the worktree alone and with its branch.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ConfirmChecks {
    pub worktree: Option<WorktreeCleanupInspection>,
    pub branch: Option<WorktreeCleanupInspection>,
}

/// TS: worktreesFlow.tsx DeleteConfirm `override`: typing DELETE to authorize data loss for this option.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DeleteOverride {
    pub delete_branch: bool,
    pub draft: String,
}

/// TS: worktreesFlow.tsx DeleteConfirm.
#[derive(Clone, Debug, PartialEq)]
pub struct DeleteConfirm {
    /// One worktree, or the ready group (`bulk`).
    pub entries: Vec<WorktreeOverviewEntry>,
    pub bulk: bool,
    pub index: usize,
    pub checks: ConfirmChecks,
    pub override_: Option<DeleteOverride>,
    /// Their notes' open checklist items stay there (space); by default they go to the backlog first.
    pub keep_open_items: bool,
}

/// TS: worktreesFlow.tsx checkedAgo (`just now` / `3m ago`).
pub fn checked_ago(ms: i64) -> String {
    let _ = ms;
    todo!("F6")
}

/// TS: worktreesFlow.tsx WorktreeLine: one row of the list.
pub fn worktree_line(
    row: &WorktreeRow,
    selected: bool,
    width: usize,
    sessions: &[&SessionRecord],
    default_branch: Option<&str>,
    now: i64,
) -> Row {
    let _ = (row, selected, width, sessions, default_branch, now);
    todo!("F6")
}

/// TS: worktreesFlow.tsx detailLines: the selected row's details under the list.
pub fn detail_lines(
    row: Option<&WorktreeRow>,
    rows: &[WorktreeRow],
    sessions: &[&SessionRecord],
    default_branch: Option<&str>,
    spinner_frame: &str,
    now: i64,
    width: usize,
) -> Vec<Row> {
    let _ = (row, rows, sessions, default_branch, spinner_frame, now, width);
    todo!("F6")
}

/// TS: worktreesFlow.tsx ConfirmPane props (width and height are the area's).
#[derive(Clone, Copy, Debug)]
pub struct ConfirmPaneProps<'a> {
    pub state: &'a DeleteConfirm,
    pub options: &'a [ConfirmOption],
    pub sessions: &'a [&'a SessionRecord],
    /// Open checklist items of their notes (sent to the backlog first unless kept).
    pub open_items: &'a [String],
    pub working: Option<&'a str>,
}

/// TS: worktreesFlow.tsx ConfirmPane.
pub fn render_confirm_pane(props: &ConfirmPaneProps<'_>, buf: &mut Buffer, area: Rect) {
    let _ = (props, buf, area);
    todo!("F6")
}

/// TS: worktreesFlow.tsx WorktreesFlowOptions (values; `onExit`, `onGoTo`, `setError`, `setStatusMessage` are
/// effects).
#[derive(Clone, Copy, Debug)]
pub struct WorktreesFlowProps<'a> {
    pub client: Option<&'a Rc<LiveClient>>,
    /// The checkout this Deckhand runs in: the repository is the one it belongs to.
    pub cwd: &'a str,
    /// Every session of the repository.
    pub sessions: &'a [&'a SessionRecord],
    pub spinner_frame: &'a str,
}

/// TS: worktreesFlow.tsx useWorktreesFlow's state.
#[derive(Debug, Default)]
pub struct WorktreesFlow {
    pub overview: Option<WorktreeOverview>,
    pub loading: bool,
    /// The selected row's key, else its position among the selectable rows.
    pub selected: (Option<String>, usize),
    pub confirm: Option<DeleteConfirm>,
    /// What is being deleted (`Deleting x… 2/5`).
    pub working: Option<String>,
    /// TS `loadId` / `checkId`: a late response for an older request is dropped.
    pub load_id: u64,
    pub check_id: u64,
    /// The list's first shown row (TS `top` ref).
    pub top: usize,
}

/// The worktree manager's keys (keysDocumented: each must be in the guide's Worktrees topic).
pub static KEYMAP: Keymap = &[];

impl WorktreesFlow {
    /// TS: WorktreesFlow.open (W): loads the list, the selection on `select_path` when listed.
    pub fn open(&mut self, select_path: Option<&str>, props: &WorktreesFlowProps<'_>, cx: &mut FlowCx<App, Self>) {
        let _ = (select_path, props, cx);
        todo!("F6")
    }

    /// TS: useWorktreesFlow load.
    pub fn load(&mut self, select_path: Option<&str>, props: &WorktreesFlowProps<'_>, cx: &mut FlowCx<App, Self>) {
        let _ = (select_path, props, cx);
        todo!("F6")
    }

    /// The rows as shown now, running sessions from the live list (TS `rows`).
    pub fn rows(&self, props: &WorktreesFlowProps<'_>, now: i64) -> Vec<WorktreeRow> {
        let _ = (props, now);
        todo!("F6")
    }

    /// TS: useWorktreesFlow openConfirm: the deletion confirmation, with fresh checks of one worktree.
    pub fn open_confirm(
        &mut self,
        entries: Vec<WorktreeOverviewEntry>,
        bulk: bool,
        props: &WorktreesFlowProps<'_>,
        cx: &mut FlowCx<App, Self>,
    ) {
        let _ = (entries, bulk, props, cx);
        todo!("F6")
    }

    /// TS: useWorktreesFlow options: the confirmation's choices.
    pub fn options(&self, state: &DeleteConfirm, props: &WorktreesFlowProps<'_>) -> Vec<ConfirmOption> {
        let _ = (state, props);
        todo!("F6")
    }

    /// TS: useWorktreesFlow deleteOne.
    pub fn delete_one(
        &mut self,
        entry: &WorktreeOverviewEntry,
        delete_branch: bool,
        allow_data_loss: bool,
        keep_open_items: bool,
        props: &WorktreesFlowProps<'_>,
        cx: &mut FlowCx<App, Self>,
    ) {
        let _ = (entry, delete_branch, allow_data_loss, keep_open_items, props, cx);
        todo!("F6")
    }

    /// TS: useWorktreesFlow deleteAll: the ready group, one at a time.
    pub fn delete_all(
        &mut self,
        entries: Vec<WorktreeOverviewEntry>,
        with_branches: bool,
        keep_open_items: bool,
        props: &WorktreesFlowProps<'_>,
        cx: &mut FlowCx<App, Self>,
    ) {
        let _ = (entries, with_branches, keep_open_items, props, cx);
        todo!("F6")
    }
}

impl Flow<App> for WorktreesFlow {
    type Props<'a> = WorktreesFlowProps<'a>;

    fn handle_input(&mut self, props: &WorktreesFlowProps<'_>, input: &Input, cx: &mut FlowCx<App, Self>) {
        let _ = (props, input, cx);
        todo!("F6")
    }

    fn render(&mut self, props: &WorktreesFlowProps<'_>, buf: &mut Buffer, area: Rect) {
        let _ = (props, buf, area);
        todo!("F6")
    }

    /// TS: WorktreesFlow.hint: the manager's footer.
    fn hint(&self, props: &WorktreesFlowProps<'_>, width: usize) -> String {
        let _ = (props, width);
        todo!("F6")
    }
}
