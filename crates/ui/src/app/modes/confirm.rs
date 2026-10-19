//! Port of app.tsx's confirmations: stopping a session (x: kill, with the worktree's deletion choices and their
//! cleanup inspections; confirm-loss: typing DELETE), merging its worktree (m: the preview, commit-first and the
//! note's open items; merge-conflicts: keep or abort) and removing an archived session for good (confirm-remove),
//! with their panes (KillConfirmPane, MergeConfirmPane, MergeConflictPane, RemoveConfirmPane).

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use deckhand_client::live::KillOptions;
use deckhand_core::key::Input;
use deckhand_core::merge_model::MergeNoteEntry;
use deckhand_protocol::data::{
    ResolveMergeAction, SessionCleanupInspection, SessionRecord, Task, WorktreeMergeMode, WorktreeMergeResult,
};

use crate::app::state::{App, MergeFlow};
use crate::keymap::Keymap;

/// confirm-kill keys.
pub static KILL_KEYMAP: Keymap = &[];
/// confirm-merge keys.
pub static MERGE_KEYMAP: Keymap = &[];
/// confirm-remove keys.
pub static REMOVE_KEYMAP: Keymap = &[];

/// TS: app.tsx KillOptionKind.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum KillOptionKind {
    Kill,
    Delete,
    DeleteBranch,
    Cancel,
}

/// TS: app.tsx KillOption.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KillOption {
    pub kind: KillOptionKind,
    pub label: &'static str,
}

/// TS: app.tsx structuralBlockers (none without an inspection).
pub fn structural_blockers(inspection: Option<&SessionCleanupInspection>) -> &[String] {
    let _ = inspection;
    todo!("A4")
}

/// TS: app.tsx killOptions: structural blockers mean the blocked deletion is not offered at all.
pub fn kill_options(
    can_delete: bool,
    can_delete_branch: bool,
    worktree_inspection: Option<&SessionCleanupInspection>,
    branch_inspection: Option<&SessionCleanupInspection>,
) -> Vec<KillOption> {
    let _ = (can_delete, can_delete_branch, worktree_inspection, branch_inspection);
    todo!("A4")
}

/// TS: app.tsx cleanupSummary.
pub fn cleanup_summary(inspection: Option<&SessionCleanupInspection>) -> &'static str {
    let _ = inspection;
    todo!("A4")
}

/// TS: x on a running session: the kill confirmation, inspecting what deleting its worktree (and branch) would lose.
pub fn open_kill_confirm(app: &mut App) {
    let _ = app;
    todo!("A4")
}

/// TS: the confirm-kill branch of the useTerminalInput handler.
pub fn handle_confirm_kill(app: &mut App, input: &Input) {
    let _ = (app, input);
    todo!("A4")
}

/// TS: the confirm-loss branch (type DELETE, then Enter).
pub fn handle_confirm_loss(app: &mut App, input: &Input) {
    let _ = (app, input);
    todo!("A4")
}

/// TS `killSelected` (a 1.5 s check says to press X when a polite stop did not end it).
pub fn kill_selected(app: &mut App, options: KillOptions) {
    let _ = (app, options);
    todo!("A4")
}

/// TS: m on a session in a worktree: the merge confirmation, its preview fetched fresh.
pub fn open_merge_confirm(app: &mut App) {
    let _ = app;
    todo!("A4")
}

/// TS: the confirm-merge branch.
pub fn handle_confirm_merge(app: &mut App, input: &Input) {
    let _ = (app, input);
    todo!("A4")
}

/// TS `mergeSelected`.
pub fn merge_selected(app: &mut App, mode: WorktreeMergeMode) {
    let _ = (app, mode);
    todo!("A4")
}

/// TS: the merge-conflicts branch (Enter/Esc keep, x aborts).
pub fn handle_merge_conflicts(app: &mut App, input: &Input) {
    let _ = (app, input);
    todo!("A4")
}

/// TS `resolveConflictedMerge`.
pub fn resolve_conflicted_merge(app: &mut App, action: ResolveMergeAction) {
    let _ = (app, action);
    todo!("A4")
}

/// TS `requestRemove` (Backspace on an archived session): always asks, listing open note items.
pub fn request_remove(app: &mut App) {
    let _ = app;
    todo!("A4")
}

/// TS: the confirm-remove branch.
pub fn handle_confirm_remove(app: &mut App, input: &Input) {
    let _ = (app, input);
    todo!("A4")
}

/// TS `removeSelected` (`move_open_items`: send the notes' open items to Tasks first).
pub fn remove_selected(app: &mut App, move_open_items: bool) {
    let _ = (app, move_open_items);
    todo!("A4")
}

/// TS: app.tsx mergeNoteEntries: the worktree's shared note first, then the session's and its sub-sessions'.
pub fn merge_note_entries(session: Option<&SessionRecord>, sessions: &[&SessionRecord]) -> Vec<MergeNoteEntry> {
    let _ = (session, sessions);
    todo!("A4")
}

/// TS: app.tsx mergeTasks: the worktree's open tasks (the merge ticks them).
pub fn merge_tasks<'a>(tasks: &'a [Task], session: Option<&SessionRecord>) -> Vec<&'a Task> {
    let _ = (tasks, session);
    todo!("A4")
}

/// TS: app.tsx hasMergedMarker.
pub fn has_merged_marker(session: Option<&SessionRecord>) -> bool {
    let _ = session;
    todo!("A4")
}

/// TS: app.tsx mergedTargetBranch.
pub fn merged_target_branch(session: &SessionRecord) -> Option<&str> {
    let _ = session;
    todo!("A4")
}

/// TS: app.tsx KillConfirmPane props (width and height are the area's).
#[derive(Clone, Copy, Debug)]
pub struct KillConfirmPaneProps<'a> {
    pub session: Option<&'a SessionRecord>,
    pub sessions: &'a [&'a SessionRecord],
    pub options: &'a [KillOption],
    pub selected_index: usize,
    pub force: bool,
    pub inspection: Option<&'a SessionCleanupInspection>,
    /// Open items of its worktree's note: sent to the backlog when it is deleted, unless kept (space).
    pub open_items: &'a [String],
    pub keep_open_items: bool,
}

/// TS: app.tsx KillConfirmPane.
pub fn render_kill_confirm_pane(props: &KillConfirmPaneProps<'_>, buf: &mut Buffer, area: Rect) {
    let _ = (props, buf, area);
    todo!("A4")
}

/// TS: app.tsx MergeConfirmPane props (width and height are the area's).
#[derive(Clone, Copy, Debug)]
pub struct MergeConfirmPaneProps<'a> {
    pub session: Option<&'a SessionRecord>,
    pub sessions: &'a [&'a SessionRecord],
    pub flow: Option<&'a MergeFlow>,
    pub selected_index: usize,
    pub tasks: &'a [Task],
}

/// TS: app.tsx MergeConfirmPane.
pub fn render_merge_confirm_pane(props: &MergeConfirmPaneProps<'_>, buf: &mut Buffer, area: Rect) {
    let _ = (props, buf, area);
    todo!("A4")
}

/// TS: app.tsx MergeConflictPane.
pub fn render_merge_conflict_pane(result: &WorktreeMergeResult, buf: &mut Buffer, area: Rect) {
    let _ = (result, buf, area);
    todo!("A4")
}

/// TS: app.tsx RemoveConfirmPane props (width is the area's).
#[derive(Clone, Copy, Debug)]
pub struct RemoveConfirmPaneProps<'a> {
    pub session: Option<&'a SessionRecord>,
    pub items: &'a [String],
}

/// TS: app.tsx RemoveConfirmPane.
pub fn render_remove_confirm_pane(props: &RemoveConfirmPaneProps<'_>, buf: &mut Buffer, area: Rect) {
    let _ = (props, buf, area);
    todo!("A4")
}
