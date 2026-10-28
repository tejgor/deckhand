//! Confirmations: stopping a session (x: kill, with the worktree's deletion choices and their cleanup inspections;
//! confirm-loss: typing DELETE), merging its worktree (m: the preview, commit-first and the note's open items;
//! merge-conflicts: keep or abort) and removing an archived session for good (confirm-remove), with their panes
//! (KillConfirmPane, MergeConfirmPane, MergeConflictPane, RemoveConfirmPane).

use std::time::Duration;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use deckhand_client::live::{KillOptions, MergeOptions};
use deckhand_core::display::{
    HintPart, compact_path, display_session_title, error_message, fit_hint, truncate, wrap_words,
};
use deckhand_core::jsstr::{js_pad_end, js_slice, js_trim};
use deckhand_core::key::Input;
use deckhand_core::merge_model::{
    MergeConfirmInput, MergeLine, MergeNoteEntry, OpenItemsToggle, conflict_view, merge_confirm_layout,
};
use deckhand_core::session_order::session_descendants;
use deckhand_core::tasks_board::{open_items_removed_with, worktree_open_items};
use deckhand_protocol::data::{
    CleanupInspection, ResolveMergeAction, SessionCleanupInspection, SessionRecord, SessionStatus, SessionWorktreeMode,
    Task, WorktreeMergeMode, WorktreeMergeResult,
};

use crate::app::sanitize_name_input;
use crate::app::state::{App, CleanupCheck, MergeConflict, MergeFlow, Mode};
use crate::app::truthy;
use crate::keymap::{Binding, HelpRef, KeySpec, Keymap, NamedKey};
use crate::panes::cleanup::cleanup_loss_lines;
use crate::panes::menu::{SelectableRowProps, selectable_row};
use crate::primitives::{
    Border, DialogRows, DialogScroll, DialogView, Row, Seg, TextStyle, Wrap, dialog_view_fitting, draw_dialog,
    draw_panel, fit_row, string_width,
};
use crate::theme::{THEME, color};

/// How long after a polite stop the session is checked to have ended (TS `setTimeout(..., 1500)`).
const STILL_RUNNING_CHECK: Duration = Duration::from_millis(1500);

/// confirm-kill keys (and confirm-loss's typing, which is not a letter binding).
pub static KILL_KEYMAP: Keymap = &[
    Binding {
        keys: &[KeySpec::Named(NamedKey::Up), KeySpec::Char("k")],
        action: "previous choice",
        help: HelpRef::Hidden("the confirmation shows its own hint line"),
    },
    Binding {
        keys: &[KeySpec::Named(NamedKey::Down), KeySpec::Char("j")],
        action: "next choice",
        help: HelpRef::Hidden("the confirmation shows its own hint line"),
    },
    Binding {
        keys: &[KeySpec::Char(" ")],
        action: "keep the note's open items in the note",
        help: HelpRef::Hidden("the confirmation shows its own hint line"),
    },
    Binding {
        keys: &[KeySpec::Named(NamedKey::Return)],
        action: "choose",
        help: HelpRef::Hidden("the confirmation shows its own hint line"),
    },
    Binding {
        keys: &[KeySpec::Named(NamedKey::Escape)],
        action: "cancel",
        help: HelpRef::Hidden("the confirmation shows its own hint line"),
    },
    crate::keymap::SCROLL_DIALOG,
];
/// confirm-merge keys (and merge-conflicts': enter / esc keep, x aborts).
pub static MERGE_KEYMAP: Keymap = &[
    Binding {
        keys: &[KeySpec::Named(NamedKey::Up), KeySpec::Char("k")],
        action: "previous row",
        help: HelpRef::Hidden("the confirmation shows its own hint line"),
    },
    Binding {
        keys: &[KeySpec::Named(NamedKey::Down), KeySpec::Char("j")],
        action: "next row",
        help: HelpRef::Hidden("the confirmation shows its own hint line"),
    },
    Binding {
        keys: &[KeySpec::Char(" ")],
        action: "commit first / keep the note's open items",
        help: HelpRef::Hidden("the confirmation shows its own hint line"),
    },
    Binding {
        keys: &[KeySpec::Named(NamedKey::Return)],
        action: "choose",
        help: HelpRef::Hidden("the confirmation shows its own hint line"),
    },
    Binding {
        keys: &[KeySpec::Named(NamedKey::Escape)],
        action: "cancel (keep the merge in progress on conflicts)",
        help: HelpRef::Hidden("the confirmation shows its own hint line"),
    },
    Binding {
        keys: &[KeySpec::Char("x")],
        action: "abort a conflicted merge",
        help: HelpRef::Hidden("the conflict screen shows its own choices"),
    },
    crate::keymap::SCROLL_DIALOG,
];
/// confirm-remove keys.
pub static REMOVE_KEYMAP: Keymap = &[
    Binding {
        keys: &[KeySpec::Named(NamedKey::Return)],
        action: "remove (moving the note's open items to Tasks first)",
        help: HelpRef::Hidden("the confirmation shows its own hint line"),
    },
    Binding {
        keys: &[KeySpec::Char("x")],
        action: "remove anyway",
        help: HelpRef::Hidden("the confirmation shows its own hint line"),
    },
    Binding {
        keys: &[KeySpec::Named(NamedKey::Escape)],
        action: "keep it",
        help: HelpRef::Hidden("the confirmation shows its own hint line"),
    },
    crate::keymap::SCROLL_DIALOG,
];

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
    inspection.map_or(&[], |inspection| inspection.structural_blockers.as_slice())
}

/// TS: app.tsx killOptions: structural blockers mean the blocked deletion is not offered at all.
///
/// Structural blockers (main/current/shared worktrees, branch changed/protected) cannot be overridden. Branch
/// deletion has its own inspection whose blockers can exceed the worktree's.
pub fn kill_options(
    can_delete: bool,
    can_delete_branch: bool,
    worktree_inspection: Option<&SessionCleanupInspection>,
    branch_inspection: Option<&SessionCleanupInspection>,
) -> Vec<KillOption> {
    let cancel = KillOption {
        kind: KillOptionKind::Cancel,
        label: "Cancel",
    };
    if !can_delete {
        return vec![
            KillOption {
                kind: KillOptionKind::Kill,
                label: "Kill session",
            },
            cancel,
        ];
    }
    let keep = KillOption {
        kind: KillOptionKind::Kill,
        label: "Kill only, keep worktree (restartable)",
    };
    if !structural_blockers(worktree_inspection).is_empty() {
        return vec![keep, cancel];
    }
    let offer_branch = can_delete_branch && structural_blockers(branch_inspection).is_empty();
    let mut options = vec![
        keep,
        KillOption {
            kind: KillOptionKind::Delete,
            label: "Kill and delete worktree (not restartable)",
        },
    ];
    if offer_branch {
        options.push(KillOption {
            kind: KillOptionKind::DeleteBranch,
            label: "Kill, delete worktree and branch (not restartable)",
        });
    }
    options.push(cancel);
    options
}

/// TS: app.tsx cleanupSummary.
pub fn cleanup_summary(inspection: Option<&SessionCleanupInspection>) -> &'static str {
    let Some(inspection) = inspection else {
        return "Checking cleanup safety…";
    };
    if !structural_blockers(Some(inspection)).is_empty() {
        return "Worktree deletion is blocked:";
    }
    if inspection.inspection.safe {
        "Local cleanup checks passed"
    } else {
        "Deleting the worktree would lose (typing DELETE overrides):"
    }
}

/// `n` and its noun, `1 file` / `2 files` (the status messages' `${n} file${n === 1 ? '' : 's'}`).
fn counted(count: i64, noun: &str) -> String {
    format!("{count} {noun}{}", if count == 1 { "" } else { "s" })
}

/// Not in the TS: the force kill's choices (`X`): it never deletes, so nothing is checked first.
pub fn force_kill_options(can_delete: bool) -> Vec<KillOption> {
    vec![
        KillOption {
            kind: KillOptionKind::Kill,
            label: if can_delete {
                "Force kill, keep worktree (restartable)"
            } else {
                "Force kill session"
            },
        },
        KillOption {
            kind: KillOptionKind::Cancel,
            label: "Cancel",
        },
    ]
}

/// TS: the options now offered by the confirmation (`killConfirmOptions`); a force kill's are its own.
pub fn current_kill_options(app: &App) -> Vec<KillOption> {
    if app.kill.force {
        return force_kill_options(app.selected_can_delete_worktree());
    }
    let check = app.current_cleanup_check();
    kill_options(
        app.selected_can_delete_worktree(),
        app.selected_can_delete_branch(),
        check.and_then(|check| check.worktree.as_ref()),
        check.and_then(|check| check.branch.as_ref()),
    )
}

/// Not in the TS: the kill confirmation is still checking what deleting the worktree (and, when it could go too, its
/// branch) would lose. No choices are shown until both answers are in: drawn early, they moved when the file list
/// arrived, and a deletion found blocked dropped its choices from under the cursor. Without a worktree nothing is
/// checked, so Kill shows at once.
pub fn kill_loading(app: &App) -> bool {
    !app.kill.force
        && app
            .current_cleanup_check()
            .is_some_and(|check| match check.worktree.as_ref() {
                None => true,
                // A blocked worktree deletion offers no delete choices, so the branch's answer does not matter.
                Some(worktree) => {
                    structural_blockers(Some(worktree)).is_empty()
                        && app.selected_can_delete_branch()
                        && check.branch.is_none()
                }
            })
}

/// Not in the TS: X on a running session: the kill confirmation with the force kill chosen (one Enter), which never
/// deletes, so nothing is inspected. It acted at once before, the Shift twin of x, which asks.
pub fn open_force_kill_confirm(app: &mut App) {
    if app
        .selected_session()
        .is_none_or(|s| s.status != SessionStatus::Running)
    {
        return;
    }
    app.kill.confirm_index = 0;
    app.kill.force = true;
    app.kill.keep_open_items = false;
    app.kill.cleanup_check = None;
    app.set_mode(Mode::ConfirmKill);
}

/// TS: x on a running session: the kill confirmation, inspecting what deleting its worktree (and branch) would lose.
pub fn open_kill_confirm(app: &mut App) {
    let Some(session) = app.selected_session().filter(|s| s.status == SessionStatus::Running) else {
        return;
    };
    let session_id = session.id.clone();
    app.kill.confirm_index = 0;
    app.kill.force = false;
    app.kill.keep_open_items = false;
    app.set_mode(Mode::ConfirmKill);
    let in_worktree = session
        .worktree
        .as_ref()
        .is_some_and(|worktree| truthy(worktree.path.as_ref()) && worktree.mode != SessionWorktreeMode::None);
    if !in_worktree {
        return;
    }
    app.kill.cleanup_request += 1;
    let request_id = app.kill.cleanup_request;
    app.kill.cleanup_check = Some(CleanupCheck {
        session_id: session_id.clone(),
        worktree: None,
        branch: None,
    });
    let Some(client) = app.client() else {
        return;
    };
    // The worktree alone, and (when the branch could go too) with its branch: each answer is kept only while it is
    // still the latest request for this session.
    let mut asks = vec![false];
    if app.selected_can_delete_branch() {
        asks.push(true);
    }
    for delete_branch in asks {
        let reply = client.inspect_cleanup(&session_id, Some(delete_branch));
        let session_id = session_id.clone();
        let task = app.dispatch().spawn(reply, move |app: &mut App, result| {
            let inspection = result.unwrap_or_else(|error| SessionCleanupInspection {
                inspection: CleanupInspection {
                    safe: false,
                    reasons: vec![error_message(&error)],
                    dirty_files: 0,
                    untracked_files: 0,
                    ignored_files: 0,
                    files: None,
                    unpublished_commits: None,
                    unmerged_commits: None,
                },
                structural_blockers: Vec::new(),
            });
            if app.kill.cleanup_request != request_id {
                return;
            }
            if let Some(check) = app
                .kill
                .cleanup_check
                .as_mut()
                .filter(|check| check.session_id == session_id)
            {
                if delete_branch {
                    check.branch = Some(inspection);
                } else {
                    check.worktree = Some(inspection);
                }
            }
        });
        drop(task);
    }
}

/// TS: the confirm-kill branch of the useTerminalInput handler.
pub fn handle_confirm_kill(app: &mut App, input: &Input) {
    let key = &input.key;
    let options = current_kill_options(app);
    let count = options.len().max(1);
    let selected_index = app.kill.confirm_index.min(options.len().saturating_sub(1));
    if key.escape {
        app.set_mode(Mode::Browse);
        return;
    }
    // Until both checks are in, the choices are not shown: only Esc acts.
    if kill_loading(app) {
        if key.return_ {
            app.set_status(Some("Still checking cleanup safety…".to_owned()));
        }
        return;
    }
    if key.up_arrow || input.input == "k" {
        app.kill.confirm_index = (selected_index + count - 1) % count;
        return;
    }
    if key.down_arrow || input.input == "j" {
        app.kill.confirm_index = (selected_index + 1) % count;
        return;
    }
    if input.input == " " {
        app.kill.keep_open_items = !app.kill.keep_open_items;
        return;
    }
    if !key.return_ {
        return;
    }
    let Some(option) = options.get(selected_index) else {
        return;
    };
    let force = app.kill.force;
    match option.kind {
        KillOptionKind::Kill => kill_selected(
            app,
            KillOptions {
                force,
                ..KillOptions::default()
            },
        ),
        KillOptionKind::Delete | KillOptionKind::DeleteBranch => {
            // Wait for this session's inspection; never authorize from another session's result.
            let delete_branch = option.kind == KillOptionKind::DeleteBranch;
            let Some(inspection) = app.cleanup_inspection_for(delete_branch).cloned() else {
                return;
            };
            let blockers = structural_blockers(Some(&inspection));
            if !blockers.is_empty() {
                app.set_error(Some(blockers.join("; ")));
            } else if inspection.inspection.safe {
                kill_selected(
                    app,
                    KillOptions {
                        delete_worktree: true,
                        delete_branch,
                        force,
                        ..KillOptions::default()
                    },
                );
            } else {
                app.kill.pending_delete_branch = delete_branch;
                app.kill.confirmation_draft.clear();
                app.review.details_scroll = 0;
                app.set_mode(Mode::ConfirmLoss);
            }
        }
        KillOptionKind::Cancel => app.set_mode(Mode::Browse),
    }
}

/// TS: the confirm-loss branch (type DELETE, then Enter).
pub fn handle_confirm_loss(app: &mut App, input: &Input) {
    let key = &input.key;
    if key.escape {
        app.set_mode(Mode::Browse);
        return;
    }
    if app.scroll_details_pane(input) {
        return;
    }
    if key.backspace || key.delete {
        // `value.slice(0, -1)`: the draft is typed text, so a character is a unit but for astral ones.
        app.kill.confirmation_draft.pop();
        return;
    }
    if key.return_ {
        if app.kill.confirmation_draft == "DELETE" {
            let delete_branch = app.kill.pending_delete_branch;
            let allowed = app.cleanup_inspection_for(delete_branch).is_some_and(|inspection| {
                !inspection.inspection.safe && structural_blockers(Some(inspection)).is_empty()
            });
            if allowed {
                let force = app.kill.force;
                kill_selected(
                    app,
                    KillOptions {
                        delete_worktree: true,
                        delete_branch,
                        force,
                        allow_data_loss: true,
                        send_open_items: false,
                    },
                );
            }
        }
        return;
    }
    if !key.ctrl && !key.meta {
        let typed = format!("{}{}", app.kill.confirmation_draft, sanitize_name_input(&input.input));
        app.kill.confirmation_draft = js_slice(&typed, 0, Some(10));
    }
}

/// TS `killSelected` (a 1.5 s check says to press X when a polite stop did not end it).
///
/// `options.send_open_items` is not read: whether the worktree note's open items go to the backlog is decided here,
/// from the confirmation's choice, as TS does.
pub fn kill_selected(app: &mut App, options: KillOptions) {
    let Some(client) = app.client() else {
        return;
    };
    let Some(session) = app.selected_session().filter(|s| s.status == SessionStatus::Running) else {
        return;
    };
    app.set_busy(true);
    app.set_error(None);
    let killed_session_id = session.id.clone();
    let deleting = options.delete_worktree || options.delete_branch;
    let worktree_id = session.worktree.as_ref().and_then(|worktree| worktree.id.as_deref());
    let send_open_items =
        deleting && !app.kill.keep_open_items && !worktree_open_items(worktree_id, &app.data.repo_sessions).is_empty();
    let reply = client.kill_session(
        &killed_session_id,
        KillOptions {
            delete_worktree: deleting,
            delete_branch: options.delete_branch,
            force: options.force,
            allow_data_loss: options.allow_data_loss,
            send_open_items,
        },
    );
    let force = options.force;
    let task = app.dispatch().spawn(reply, move |app: &mut App, result| {
        match result {
            Ok(()) => {
                app.set_mode(Mode::Browse);
                if !force {
                    // Not cancelled by a later kill (TS never clears it): each check stands alone.
                    app.dispatch()
                        .after(STILL_RUNNING_CHECK, move |app: &mut App| {
                            let running = app
                                .sessions()
                                .iter()
                                .any(|item| item.id == killed_session_id && item.status == SessionStatus::Running);
                            if running {
                                app.set_error(Some("session still running; press X to force kill".to_owned()));
                            }
                        })
                        .detach();
                }
            }
            Err(error) => app.set_error(Some(error_message(&error))),
        }
        app.set_busy(false);
    });
    drop(task);
}

/// TS: m on a session in a worktree: the merge confirmation, its preview fetched fresh.
pub fn open_merge_confirm(app: &mut App) {
    let Some(session) = app.selected_session() else {
        return;
    };
    if !session
        .worktree
        .as_ref()
        .is_some_and(|worktree| truthy(worktree.path.as_ref()))
    {
        return;
    }
    let session_id = session.id;
    app.merge.request += 1;
    let request_id = app.merge.request;
    app.merge.confirm_index = 0;
    app.merge.flow = Some(MergeFlow {
        session_id: session_id.clone(),
        commit_first: true,
        ..MergeFlow::default()
    });
    app.set_mode(Mode::ConfirmMerge);
    // What would be merged, checked fresh each time the confirmation opens.
    let Some(client) = app.client() else {
        return;
    };
    let reply = client.merge_preview(&session_id, &app.props.cwd);
    let task = app.dispatch().spawn(reply, move |app: &mut App, result| {
        if app.merge.request != request_id {
            return;
        }
        if let Some(flow) = app.merge.flow.as_mut().filter(|flow| flow.session_id == session_id) {
            match result {
                Ok(preview) => flow.preview = Some(preview),
                Err(error) => flow.preview_error = Some(error_message(&error)),
            }
        }
    });
    drop(task);
}

/// TS: the confirm-merge branch.
pub fn handle_confirm_merge(app: &mut App, input: &Input) {
    let key = &input.key;
    const OPTION_COUNT: usize = 3;
    if key.escape {
        app.set_mode(Mode::Browse);
        return;
    }
    // Until the preview (or its error) is in, the choices are not shown: only Esc acts.
    if merge_loading(app.merge.flow.as_ref()) {
        if key.return_ {
            app.set_status(Some("Still checking what will be merged…".to_owned()));
        }
        return;
    }
    // The note's open items row (listed once the preview is in) comes before the options in one j/k cycle; space or
    // enter on it switches it, space elsewhere toggles commit-first.
    let has_open_items = {
        let sessions = app.sessions();
        let worktree_id = app
            .selected_session()
            .and_then(|session| session.worktree)
            .and_then(|worktree| worktree.id);
        !worktree_open_items(worktree_id.as_deref(), &sessions).is_empty()
    };
    let items_row = usize::from(app.merge.flow.as_ref().is_some_and(|flow| flow.preview.is_some()) && has_open_items);
    let on_items = items_row == 1 && app.merge.flow.as_ref().is_some_and(|flow| flow.on_items);
    if (input.input == " " || key.return_) && on_items {
        if let Some(flow) = app.merge.flow.as_mut() {
            flow.keep_open_items = !flow.keep_open_items;
        }
        return;
    }
    if input.input == " " {
        if let Some(flow) = app.merge.flow.as_mut()
            && flow.preview.as_ref().is_some_and(|preview| preview.uncommitted != 0)
        {
            flow.commit_first = !flow.commit_first;
        }
        return;
    }
    let step: isize = if key.up_arrow || input.input == "k" {
        -1
    } else if key.down_arrow || input.input == "j" {
        1
    } else {
        0
    };
    if step != 0 {
        let count = items_row + OPTION_COUNT;
        let current = if on_items {
            0
        } else {
            items_row + app.merge.confirm_index
        };
        // ((current + step + count) % count) on non-negative numbers.
        let at = (current + count).saturating_add_signed(step) % count;
        if let Some(flow) = app.merge.flow.as_mut() {
            flow.on_items = at < items_row;
        }
        if at >= items_row {
            app.merge.confirm_index = at - items_row;
        }
        return;
    }
    if key.return_ {
        match app.merge.confirm_index {
            0 => merge_selected(app, WorktreeMergeMode::Merge),
            1 => merge_selected(app, WorktreeMergeMode::Squash),
            _ => app.set_mode(Mode::Browse),
        }
    }
}

/// Not in the TS: the merge screen is still checking what will be merged (neither the preview nor its error is in),
/// so it shows no choices yet: they would move down when the commits and summary arrive.
pub fn merge_loading(flow: Option<&MergeFlow>) -> bool {
    flow.is_none_or(|flow| flow.preview.is_none() && !truthy(flow.preview_error.as_ref()))
}

/// TS `mergeSelected`.
pub fn merge_selected(app: &mut App, mode: WorktreeMergeMode) {
    let Some(client) = app.client() else {
        return;
    };
    let Some(session) = app.selected_session() else {
        return;
    };
    let Some(worktree) = session
        .worktree
        .as_ref()
        .filter(|worktree| truthy(worktree.path.as_ref()) && worktree.mode != SessionWorktreeMode::None)
    else {
        return;
    };
    let session_id = session.id.clone();
    let flow = app.merge.flow.clone().filter(|flow| flow.session_id == session_id);
    // The toggle only applies when there is something uncommitted to commit.
    let commit_first = flow
        .as_ref()
        .and_then(|flow| flow.preview.as_ref())
        .is_some_and(|preview| preview.uncommitted != 0)
        && flow.as_ref().is_none_or(|flow| flow.commit_first);
    // The note's open items go to the backlog with a clean merge unless the confirmation kept them in the note.
    let send_open_items = !worktree_open_items(worktree.id.as_deref(), &app.data.repo_sessions).is_empty()
        && !flow.as_ref().is_some_and(|flow| flow.keep_open_items);
    app.set_busy(true);
    app.set_error(None);
    if let Some(flow) = app.merge.flow.as_mut().filter(|flow| flow.session_id == session_id) {
        flow.error = None;
    }
    let reply = client.merge_worktree(
        &session_id,
        mode,
        &app.props.cwd,
        MergeOptions {
            commit_first,
            send_open_items,
        },
    );
    let task = app.dispatch().spawn(reply, move |app: &mut App, result| {
        match result {
            Ok(result) => merged(app, session_id, mode, result),
            // Shown on the confirmation (a failed commit's hook output can be long); nothing was merged.
            Err(error) => {
                if let Some(flow) = app.merge.flow.as_mut().filter(|flow| flow.session_id == session_id) {
                    flow.error = Some(error_message(&error));
                }
            }
        }
        app.set_busy(false);
    });
    drop(task);
}

/// TS mergeSelected after the merge answered: the conflicts screen, or the outcome in the footer.
fn merged(app: &mut App, session_id: String, mode: WorktreeMergeMode, result: WorktreeMergeResult) {
    let sent = match result.sent_items {
        Some(count) if count != 0 => format!(" · {} sent to the backlog", counted(count, "open note item")),
        _ => String::new(),
    };
    let committed = result
        .committed
        .as_ref()
        .map(|committed| format!("Committed {}, then ", counted(committed.files, "file")));
    if result.conflicted == Some(true) {
        app.merge.conflict = Some(MergeConflict { session_id, result });
        app.set_mode(Mode::MergeConflicts);
        return;
    }
    app.set_mode(Mode::Browse);
    if result.skipped == Some(true) {
        app.set_status(Some(format!(
            "Skipped merge: no new commits from {} into {}",
            result.source_ref, result.target_branch
        )));
        return;
    }
    let applied = format!(
        "{} from {} into {}",
        if mode == WorktreeMergeMode::Squash {
            "squash applied"
        } else {
            "merge applied without commit"
        },
        result.source_ref,
        result.target_branch
    );
    let lead = match committed {
        Some(committed) => format!("{committed}{applied}"),
        None => {
            let mut chars = applied.chars();
            chars.next().map_or_else(String::new, |first| {
                format!("{}{}", first.to_uppercase(), chars.as_str())
            })
        }
    };
    app.set_status(Some(format!("{lead}{sent}")));
}

/// TS: the merge-conflicts branch (Enter/Esc keep, x aborts).
///
/// Only two choices: keep the merge in progress (marked merged), or abort it. Esc keeps it: nothing is undone.
pub fn handle_merge_conflicts(app: &mut App, input: &Input) {
    if input.key.return_ || input.key.escape {
        resolve_conflicted_merge(app, ResolveMergeAction::Keep);
    } else if input.input == "x" {
        resolve_conflicted_merge(app, ResolveMergeAction::Abort);
    }
}

/// TS `resolveConflictedMerge`.
pub fn resolve_conflicted_merge(app: &mut App, action: ResolveMergeAction) {
    let Some(client) = app.client() else {
        return;
    };
    let Some(MergeConflict { session_id, result }) = app.merge.conflict.clone() else {
        return;
    };
    app.set_busy(true);
    app.set_error(None);
    let reply = client.resolve_merge(&session_id, &app.props.cwd, action);
    let task = app.dispatch().spawn(reply, move |app: &mut App, outcome| {
        match outcome {
            Ok(updated) => {
                app.upsert(updated);
                app.merge.conflict = None;
                app.set_mode(Mode::Browse);
                let listed = result
                    .conflicts
                    .as_ref()
                    .map(|conflicts| i64::try_from(conflicts.len()).unwrap_or(i64::MAX));
                let count = result.conflict_count.or(listed).unwrap_or(0);
                app.set_status(Some(if action == ResolveMergeAction::Keep {
                    format!(
                        "Merge kept with conflicts in {} into {}: resolve them, then commit (marked merged)",
                        counted(count, "file"),
                        result.target_branch
                    )
                } else {
                    format!("Merge aborted; {} is as it was", result.target_branch)
                }));
            }
            Err(error) => app.set_error(Some(error_message(&error))),
        }
        app.set_busy(false);
    });
    drop(task);
}

/// TS `requestRemove` (Backspace on an archived session): always asks, listing open note items.
///
/// Removing it deletes its note (and its worktree's, when it is the last there), so it always asks; open items are
/// listed and can go to Tasks first.
pub fn request_remove(app: &mut App) {
    let Some(session) = app.selected_session().filter(|s| s.status == SessionStatus::Exited) else {
        return;
    };
    app.remove.items = open_items_removed_with(&session, &app.data.repo_sessions);
    app.set_mode(Mode::ConfirmRemove);
}

/// TS: the confirm-remove branch.
pub fn handle_confirm_remove(app: &mut App, input: &Input) {
    if input.key.escape {
        app.set_mode(Mode::Browse);
        return;
    }
    // Enter removes it, keeping open items as tasks (the safe choice when there are any); x removes them with the
    // notes.
    if input.key.return_ {
        app.set_mode(Mode::Browse);
        let move_open_items = !app.remove.items.is_empty();
        remove_selected(app, move_open_items);
        return;
    }
    if input.input == "x" && !app.remove.items.is_empty() {
        app.set_mode(Mode::Browse);
        remove_selected(app, false);
    }
}

/// TS `removeSelected` (`move_open_items`: send the notes' open items to Tasks first).
pub fn remove_selected(app: &mut App, move_open_items: bool) {
    let Some(client) = app.client() else {
        return;
    };
    let Some(session) = app.selected_session().filter(|s| s.status == SessionStatus::Exited) else {
        return;
    };
    app.set_busy(true);
    app.set_error(None);
    let reply = client.remove_session(&session.id, Some(move_open_items));
    let task = app.dispatch().spawn(reply, move |app: &mut App, result| {
        match result {
            Ok(()) => {
                if move_open_items {
                    app.set_status(Some("Moved its open note items to Tasks (b)".to_owned()));
                }
                app.reset_panes();
            }
            Err(error) => app.set_error(Some(error_message(&error))),
        }
        app.set_busy(false);
    });
    drop(task);
}

/// TS: app.tsx mergeNoteEntries: the worktree's shared note first, then the session's and its sub-sessions'.
pub fn merge_note_entries(session: Option<&SessionRecord>, sessions: &[&SessionRecord]) -> Vec<MergeNoteEntry> {
    let lines_of = |text: &str| -> Vec<String> {
        js_trim(text)
            .split('\n')
            .filter(|line| !line.is_empty())
            .map(str::to_owned)
            .collect()
    };
    let mut note_sessions: Vec<&SessionRecord> = Vec::new();
    if let Some(session) = session {
        note_sessions.push(session);
        note_sessions.extend(session_descendants(&session.id, sessions).into_iter().copied());
    }
    let mut entries: Vec<MergeNoteEntry> = note_sessions
        .into_iter()
        .map(|note_session| MergeNoteEntry {
            key: note_session.id.clone(),
            title: display_session_title(note_session, sessions),
            lines: lines_of(note_session.notes.as_deref().unwrap_or("")),
        })
        .filter(|entry| !entry.lines.is_empty())
        .collect();
    // The worktree's shared note comes first: it describes the worktree being merged.
    if let Some(session) = session {
        let worktree_lines = lines_of(session.shared_notes.as_ref().map_or("", |note| note.text.as_str()));
        if !worktree_lines.is_empty() {
            let branch = session
                .worktree
                .as_ref()
                .and_then(|worktree| worktree.branch.as_deref())
                .filter(|branch| !branch.is_empty());
            entries.insert(
                0,
                MergeNoteEntry {
                    key: "worktree".to_owned(),
                    title: format!(
                        "Worktree notes{}",
                        branch.map_or_else(String::new, |branch| format!(" · {branch}"))
                    ),
                    lines: worktree_lines,
                },
            );
        }
    }
    entries
}

/// TS: app.tsx mergeTasks: the worktree's open tasks (the merge ticks them).
pub fn merge_tasks<'a>(tasks: &'a [Task], session: Option<&SessionRecord>) -> Vec<&'a Task> {
    let worktree_id = session
        .and_then(|session| session.worktree.as_ref())
        .and_then(|worktree| worktree.id.as_deref());
    tasks
        .iter()
        .filter(|task| !task.done && truthy(task.meta.wt.as_ref()) && task.meta.wt.as_deref() == worktree_id)
        .collect()
}

/// TS: app.tsx hasMergedMarker.
pub fn has_merged_marker(session: Option<&SessionRecord>) -> bool {
    session.is_some_and(|session| {
        truthy(
            session
                .worktree
                .as_ref()
                .and_then(|worktree| worktree.merged_at.as_ref()),
        ) || truthy(session.merged_at.as_ref())
    })
}

/// TS: app.tsx mergedTargetBranch.
pub fn merged_target_branch(session: &SessionRecord) -> Option<&str> {
    session
        .worktree
        .as_ref()
        .and_then(|worktree| worktree.merge_target_branch.as_deref())
        .or(session.merge_target_branch.as_deref())
}

/// A single-style `<Text>` row.
fn text(text: impl Into<String>, style: TextStyle) -> Row {
    vec![Seg::new(text, style)]
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
    /// Not in the TS: the cleanup checks are not all in yet, so no choices are shown (see [`kill_loading`]).
    pub loading: bool,
    /// Not in the TS: the other deletion's check (with or without the branch), which the selection can switch to:
    /// the dialog is sized for the bigger of the two.
    pub other_inspection: Option<&'a SessionCleanupInspection>,
}

/// TS: app.tsx KillConfirmPane, as a dialog `width` wide: the question and the worktree's path on top, what
/// deleting it would lose (every file) in the body, the choices and the hint at the bottom.
pub fn kill_dialog_rows(props: &KillConfirmPaneProps<'_>, width: u16) -> DialogRows {
    let content_width = usize::from(width).saturating_sub(4).max(1);
    let mut rows = DialogRows::default();
    let session = props.session;
    let worktree = session.and_then(|session| session.worktree.as_ref());
    let title = format!(
        "{} {}?",
        if props.force { "Force kill" } else { "Kill" },
        session.map_or_else(
            || "session".to_owned(),
            |session| format!("\"{}\"", display_session_title(session, props.sessions))
        )
    );
    rows.header.extend(fit_row(
        &text(title, TextStyle::fg(THEME.error).bold(true)),
        content_width,
        Wrap::Wrap,
    ));
    if let Some(path) = worktree
        .and_then(|worktree| worktree.path.as_deref())
        .filter(|path| !path.is_empty())
    {
        rows.header.extend(fit_row(
            &text(
                truncate(&compact_path(path, content_width), content_width),
                TextStyle::fg(THEME.muted),
            ),
            content_width,
            Wrap::Wrap,
        ));
    }
    // A force kill never deletes: nothing to check.
    if !props.force
        && worktree.is_some_and(|worktree| truthy(worktree.path.as_ref()) && worktree.mode != SessionWorktreeMode::None)
    {
        rows.body.push(Vec::new());
        rows.body.extend(fit_row(
            &text(cleanup_summary(props.inspection), TextStyle::fg(THEME.warn)),
            content_width,
            Wrap::Wrap,
        ));
    }
    // What deleting would lose, in full: the reasons, then every file.
    if let Some(inspection) = props.inspection
        && !inspection.inspection.safe
        && structural_blockers(Some(inspection)).is_empty()
    {
        rows.body
            .extend(cleanup_loss_lines(&inspection.inspection, content_width, usize::MAX));
    }
    for blocker in structural_blockers(props.inspection) {
        rows.body.extend(fit_row(
            &text(format!("  {blocker}"), TextStyle::fg(THEME.warn)),
            content_width,
            Wrap::Wrap,
        ));
    }
    let footer = &mut rows.footer;
    footer.push(Vec::new());
    if props.loading {
        footer.push(text("esc cancel", TextStyle::fg(THEME.muted)));
        return rows;
    }
    for (index, option) in props.options.iter().enumerate() {
        footer.extend(selectable_row(
            &SelectableRowProps {
                selected: index == props.selected_index,
                text: option.label,
                selected_color: Some(if option.kind == KillOptionKind::Cancel {
                    THEME.muted
                } else {
                    THEME.error
                }),
                wrap: true,
                ..SelectableRowProps::default()
            },
            content_width,
        ));
    }
    let deletes = props
        .options
        .iter()
        .any(|option| matches!(option.kind, KillOptionKind::Delete | KillOptionKind::DeleteBranch));
    if !props.open_items.is_empty() && deletes {
        let keep = props.keep_open_items;
        let count = i64::try_from(props.open_items.len()).unwrap_or(i64::MAX);
        footer.extend(fit_row(
            &text(
                format!(
                    "{} If deleted: {} {}: {}",
                    if keep { "☐" } else { "☑" },
                    counted(count, "open note item"),
                    if keep {
                        "stay in its (read-only) note"
                    } else {
                        "→ backlog"
                    },
                    props.open_items.join(", ")
                ),
                TextStyle::fg(if keep { THEME.muted } else { THEME.active }),
            ),
            content_width,
            Wrap::Wrap,
        ));
    }
    footer.push(Vec::new());
    let mut hint: Vec<HintPart> = vec!["enter choose".into()];
    if !props.open_items.is_empty() {
        hint.push("space note items".into());
    }
    hint.push("esc cancel".into());
    footer.extend(fit_row(
        &text(fit_hint(&hint, content_width, " • "), TextStyle::fg(THEME.muted)),
        content_width,
        Wrap::TruncateEnd,
    ));
    rows
}

/// [`kill_dialog_rows`] drawn over `area` as `view` keeps it.
pub fn render_kill_dialog(
    props: &KillConfirmPaneProps<'_>,
    buf: &mut Buffer,
    area: Rect,
    view: DialogView,
) -> DialogScroll {
    let build = |width| kill_dialog_rows(props, width);
    // Moving between deleting the worktree and deleting it with its branch shows the other check: open at the size
    // of the bigger, so it never resizes.
    let view = match props.other_inspection.filter(|_| !props.loading) {
        Some(other) => {
            let other = KillConfirmPaneProps {
                inspection: Some(other),
                ..*props
            };
            dialog_view_fitting(area, &|width| kill_dialog_rows(&other, width), view)
        }
        None => view,
    };
    draw_dialog(buf, area, &Border::round(THEME.border_danger), &build, view)
}

/// TS: app.tsx KillConfirmPane ([`render_kill_dialog`] at the top).
pub fn render_kill_confirm_pane(props: &KillConfirmPaneProps<'_>, buf: &mut Buffer, area: Rect) {
    render_kill_dialog(props, buf, area, DialogView::default());
}

/// TS: app.tsx MergeConfirmPane props (width and height are the area's).
#[derive(Clone, Copy, Debug)]
pub struct MergeConfirmPaneProps<'a> {
    pub session: Option<&'a SessionRecord>,
    pub sessions: &'a [&'a SessionRecord],
    pub flow: Option<&'a MergeFlow>,
    pub selected_index: usize,
    pub tasks: &'a [Task],
    /// The time now (epoch milliseconds), for the commits' ages; `None` leaves them out.
    pub now: Option<i64>,
}

/// A merge line as a `<Text wrap="truncate-end">`.
fn merge_line_row(line: &MergeLine) -> Row {
    let mut style = TextStyle::default().color(line.color.and_then(color));
    style.bold = line.bold;
    text(line.text.clone(), style)
}

/// The rows the merge screen lays out for: enough that nothing (commits, a hook's output, notes) is cut; the body
/// scrolls instead.
const MERGE_LAYOUT_ROWS: usize = 100_000;

/// TS: app.tsx MergeConfirmPane, laid out `width` wide: the question, target, summary and what decides the merge
/// on top; the commits, a failed attempt's output and the notes in the body (scrolling); the open-items toggle, the
/// choices and the hint at the bottom.
pub fn merge_pane_rows(props: &MergeConfirmPaneProps<'_>, width: u16) -> DialogRows {
    let content_width = usize::from(width).saturating_sub(4).max(1);
    let session = props.session;
    let flow = props.flow;
    let started = merge_tasks(props.tasks, session);
    let layout = merge_confirm_layout(&MergeConfirmInput {
        title: session.map_or_else(
            || "worktree".to_owned(),
            |session| display_session_title(session, props.sessions),
        ),
        preview: flow.and_then(|flow| flow.preview.clone()),
        preview_error: flow.and_then(|flow| flow.preview_error.clone()),
        commit_first: flow.is_none_or(|flow| flow.commit_first),
        commit_message: session.map_or_else(String::new, |session| js_trim(&session.title).to_owned()),
        error: flow.and_then(|flow| flow.error.clone()),
        notes: merge_note_entries(session, props.sessions),
        tasks: Some(started.iter().map(|task| task.title.clone()).collect()),
        open_items: Some(OpenItemsToggle {
            titles: worktree_open_items(
                session
                    .and_then(|session| session.worktree.as_ref())
                    .and_then(|worktree| worktree.id.as_deref()),
                props.sessions,
            ),
            send: !flow.is_some_and(|flow| flow.keep_open_items),
        }),
        width: content_width,
        height: MERGE_LAYOUT_ROWS,
        now: props.now,
    });
    let on_task = flow.is_some_and(|flow| flow.on_items);
    let mut rows = DialogRows::default();
    rows.header.extend(fit_row(
        &text(layout.title.clone(), TextStyle::fg(THEME.accent).bold(true)),
        content_width,
        Wrap::TruncateEnd,
    ));
    // The target, the summary and what decides the merge (the uncommitted toggle, the task, the warnings) stay on
    // top; the commit subjects, a failed attempt's output and the notes scroll below them.
    let detail_rows = |lines: &[MergeLine]| -> Vec<Row> {
        lines
            .iter()
            .flat_map(|line| fit_row(&merge_line_row(line), content_width, Wrap::TruncateEnd))
            .collect()
    };
    let commits = layout.commit_rows.clone();
    rows.header
        .extend(detail_rows(layout.details.get(..commits.start).unwrap_or_default()));
    rows.body
        .extend(detail_rows(layout.details.get(commits.clone()).unwrap_or_default()));
    rows.header
        .extend(detail_rows(layout.details.get(commits.end..).unwrap_or_default()));
    // A failed attempt's output and the notes wrap: every line in full.
    for section in [&layout.error, &layout.notes] {
        if !section.is_empty() {
            rows.body.push(Vec::new());
            for line in section {
                rows.body
                    .extend(fit_row(&merge_line_row(line), content_width, Wrap::Wrap));
            }
        }
    }
    let footer = &mut rows.footer;
    footer.push(Vec::new());
    if merge_loading(flow) {
        footer.push(text("esc cancel", TextStyle::fg(THEME.muted)));
        return rows;
    }
    for toggle in &layout.toggles {
        footer.extend(selectable_row(
            &SelectableRowProps {
                selected: on_task,
                text: toggle,
                color: Some(if toggle.starts_with('☑') {
                    THEME.success
                } else {
                    THEME.muted
                }),
                ..SelectableRowProps::default()
            },
            content_width,
        ));
    }
    if !layout.toggles.is_empty() {
        footer.push(Vec::new());
    }
    for (index, option) in layout.options.iter().enumerate() {
        footer.extend(selectable_row(
            &SelectableRowProps {
                selected: !on_task && index == props.selected_index,
                text: option,
                selected_color: Some(if option == "Cancel" { THEME.muted } else { THEME.active }),
                ..SelectableRowProps::default()
            },
            content_width,
        ));
    }
    footer.push(Vec::new());
    footer.extend(fit_row(
        &text(layout.hint.clone(), TextStyle::fg(THEME.muted)),
        content_width,
        Wrap::TruncateEnd,
    ));
    rows
}

/// [`merge_pane_rows`] as the full-pane screen in `area` (a review, not a quick choice: it takes the room it can),
/// its body scrolled to `scroll`.
pub fn render_merge_pane(
    props: &MergeConfirmPaneProps<'_>,
    buf: &mut Buffer,
    area: Rect,
    scroll: usize,
) -> DialogScroll {
    let rows = merge_pane_rows(props, area.width);
    draw_panel(buf, area, &Border::round(THEME.border_active), &rows, scroll)
}

/// TS: app.tsx MergeConfirmPane ([`render_merge_pane`] at the top).
pub fn render_merge_confirm_pane(props: &MergeConfirmPaneProps<'_>, buf: &mut Buffer, area: Rect) {
    render_merge_pane(props, buf, area, 0);
}

/// TS: app.tsx MergeConflictPane, laid out `width` wide: the title, every conflicted file (the body), the choices.
pub fn conflict_pane_rows(result: &WorktreeMergeResult, width: u16) -> DialogRows {
    let content_width = usize::from(width).saturating_sub(4).max(1);
    let view = conflict_view(result, content_width);
    let mut rows = DialogRows::default();
    rows.header.extend(fit_row(
        &text(view.title.clone(), TextStyle::fg(THEME.warn).bold(true)),
        content_width,
        Wrap::TruncateEnd,
    ));
    for file in &view.files {
        rows.body.extend(fit_row(
            &text(file.clone(), TextStyle::fg(THEME.muted)),
            content_width,
            Wrap::TruncateEnd,
        ));
    }
    rows.footer.push(Vec::new());
    for choice in &view.choices {
        let room = content_width.saturating_sub(6).max(1);
        rows.footer.extend(fit_row(
            &[
                Seg::new(js_pad_end(&choice.key, 6, " "), TextStyle::fg(THEME.active).bold(true)),
                Seg::plain(truncate(&choice.text, room)),
            ],
            content_width,
            Wrap::TruncateEnd,
        ));
    }
    rows
}

/// [`conflict_pane_rows`] as the full-pane screen in `area` (it follows the merge screen), its body scrolled to
/// `scroll`.
pub fn render_conflict_pane(result: &WorktreeMergeResult, buf: &mut Buffer, area: Rect, scroll: usize) -> DialogScroll {
    let rows = conflict_pane_rows(result, area.width);
    draw_panel(buf, area, &Border::round(THEME.warn), &rows, scroll)
}

/// TS: app.tsx MergeConflictPane ([`render_conflict_pane`] at the top).
pub fn render_merge_conflict_pane(result: &WorktreeMergeResult, buf: &mut Buffer, area: Rect) {
    render_conflict_pane(result, buf, area, 0);
}

/// TS: app.tsx RemoveConfirmPane props (width is the area's).
#[derive(Clone, Copy, Debug)]
pub struct RemoveConfirmPaneProps<'a> {
    pub session: Option<&'a SessionRecord>,
    pub items: &'a [String],
}

/// `text` wrapped at `width` after `prefix`, later rows indented to line up under its first.
fn hanging(prefix: &str, text: &str, style: TextStyle, width: usize) -> Vec<Row> {
    let indent = " ".repeat(string_width(prefix));
    wrap_words(text, width.saturating_sub(indent.len()).max(1))
        .into_iter()
        .enumerate()
        .map(|(index, line)| {
            let lead = if index == 0 { prefix } else { indent.as_str() };
            vec![Seg::new(format!("{lead}{line}"), style)]
        })
        .collect()
}

/// TS: app.tsx RemoveConfirmPane, as a dialog `width` wide.
///
/// Removing a session for good always asks (Backspace on an archived one): it cannot be resumed afterwards and its
/// notes go to notes/trash. Every open checklist item in them is listed, and Enter sends them to Tasks first.
pub fn remove_dialog_rows(props: &RemoveConfirmPaneProps<'_>, width: u16) -> DialogRows {
    let content_width = usize::from(width).saturating_sub(4).max(1);
    let items = props.items;
    let mut rows = DialogRows::default();
    rows.header.extend(fit_row(
        &text(
            format!(
                "Remove “{}” for good?",
                props.session.map_or("session", |session| session.title.as_str())
            ),
            TextStyle::fg(THEME.error).bold(true),
        ),
        content_width,
        Wrap::Wrap,
    ));
    rows.body.push(Vec::new());
    rows.body.extend(fit_row(
        &text(
            "It cannot be resumed afterwards; its notes go to notes/trash.",
            TextStyle::default(),
        ),
        content_width,
        Wrap::Wrap,
    ));
    if !items.is_empty() {
        let count = i64::try_from(items.len()).unwrap_or(i64::MAX);
        rows.body.push(vec![
            Seg::plain("Its notes have "),
            Seg::new(
                format!("{} unchecked item{}", count, if count == 1 { "" } else { "s" }),
                TextStyle::fg(THEME.warn),
            ),
            Seg::plain(":"),
        ]);
    }
    for item in items {
        rows.body
            .extend(hanging("  ☐ ", item, TextStyle::default(), content_width));
    }
    rows.footer.push(Vec::new());
    let hint: Vec<HintPart> = if items.is_empty() {
        vec!["enter remove".into(), "esc keep it".into()]
    } else {
        vec![
            HintPart {
                text: "enter move them to Tasks, then remove".to_owned(),
                short: Some("enter move to Tasks".to_owned()),
                drop: None,
            },
            "x remove anyway".into(),
            "esc cancel".into(),
        ]
    };
    rows.footer
        .push(text(fit_hint(&hint, content_width, " • "), TextStyle::fg(THEME.muted)));
    rows
}

/// [`remove_dialog_rows`] drawn over `area` as `view` keeps it.
pub fn render_remove_dialog(
    props: &RemoveConfirmPaneProps<'_>,
    buf: &mut Buffer,
    area: Rect,
    view: DialogView,
) -> DialogScroll {
    let build = |width| remove_dialog_rows(props, width);
    draw_dialog(buf, area, &Border::round(THEME.border_danger), &build, view)
}

/// TS: app.tsx RemoveConfirmPane ([`render_remove_dialog`] at the top).
pub fn render_remove_confirm_pane(props: &RemoveConfirmPaneProps<'_>, buf: &mut Buffer, area: Rect) {
    render_remove_dialog(props, buf, area, DialogView::default());
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use serde_json::{Value, json};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::sync::mpsc::UnboundedReceiver;

    use deckhand_client::live::{LiveClient, LiveClientHandlers};
    use deckhand_core::key::Key;
    use deckhand_protocol::data::{MergePreview, SessionWorktreeRecord, SharedNote, SharedNoteKind};

    use super::*;
    use crate::app::event_loop::Update;
    use crate::app::state::AppProps;
    use crate::app::test_support::{app, session};
    use crate::primitives::{dialog_rows, string_width};

    fn run(test: impl std::future::Future<Output = ()>) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        tokio::task::LocalSet::new().block_on(&runtime, test);
    }

    fn press(text: &str) -> Input {
        Input::text(text)
    }

    fn named(update: impl FnOnce(&mut Key)) -> Input {
        let mut key = Key::NONE;
        update(&mut key);
        Input::key("", key)
    }

    fn enter() -> Input {
        named(|key| key.return_ = true)
    }

    fn escape() -> Input {
        named(|key| key.escape = true)
    }

    /// A session in a worktree at `path`.
    fn in_worktree(mut record: SessionRecord, id: &str, path: &str, branch: &str) -> SessionRecord {
        let mut worktree = SessionWorktreeRecord::new(SessionWorktreeMode::Managed);
        worktree.id = Some(id.to_owned());
        worktree.path = Some(path.to_owned());
        worktree.branch = Some(branch.to_owned());
        record.worktree = Some(worktree);
        record
    }

    fn worktree_note(id: &str, text: &str) -> SharedNote {
        SharedNote {
            kind: SharedNoteKind::Worktree,
            id: id.to_owned(),
            path: "/n".to_owned(),
            text: text.to_owned(),
            revision: "r".to_owned(),
            too_large: None,
            read_only: None,
        }
    }

    fn inspection(safe: bool, reasons: &[&str], blockers: &[&str]) -> SessionCleanupInspection {
        SessionCleanupInspection {
            inspection: CleanupInspection {
                safe,
                reasons: reasons.iter().map(|reason| (*reason).to_owned()).collect(),
                dirty_files: 0,
                untracked_files: 0,
                ignored_files: 0,
                files: None,
                unpublished_commits: None,
                unmerged_commits: None,
            },
            structural_blockers: blockers.iter().map(|blocker| (*blocker).to_owned()).collect(),
        }
    }

    /// An app whose sessions are `sessions`, the first one selected.
    fn app_with(sessions: Vec<SessionRecord>) -> (App, UnboundedReceiver<Update<App>>) {
        let (mut app, rx) = app(AppProps::default());
        app.browse.selected_id = sessions.first().map(|session| session.id.clone());
        app.data.repo_sessions = sessions;
        (app, rx)
    }

    type Requests = Rc<RefCell<Vec<Value>>>;

    /// A client over a socket pair whose daemon answers each request with `respond`'s fields (merged into the
    /// response message) and records it.
    fn fake_daemon(respond: impl Fn(&Value) -> Value + 'static) -> (Rc<LiveClient>, Requests) {
        let (socket, server) = tokio::net::UnixStream::pair().unwrap();
        let client = Rc::new(LiveClient::new(socket, LiveClientHandlers::default()));
        let requests: Requests = Rc::default();
        let log = Rc::clone(&requests);
        let (read, mut write) = server.into_split();
        tokio::task::spawn_local(async move {
            let mut lines = BufReader::new(read);
            loop {
                let mut line = String::new();
                if lines.read_line(&mut line).await.unwrap_or(0) == 0 {
                    break;
                }
                let Ok(request) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                let mut message = json!({"type": "response", "requestId": request["requestId"]});
                if let (Some(message), Some(body)) = (message.as_object_mut(), respond(&request).as_object()) {
                    message.extend(body.clone());
                }
                log.borrow_mut().push(request);
                let _ = write.write_all(format!("{message}\n").as_bytes()).await;
            }
        });
        (client, requests)
    }

    /// Lets the daemon answer and applies the updates that follow.
    async fn settle(app: &mut App, rx: &mut UnboundedReceiver<Update<App>>) {
        for _ in 0..8 {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            while let Ok(update) = rx.try_recv() {
                update(app);
            }
        }
    }

    // ── Pure helpers ─────────────────────────────────────────────────────────────────────────────────────────

    #[test]
    fn kill_options_offer_only_what_is_allowed() {
        let labels = |options: Vec<KillOption>| options.iter().map(|option| option.label).collect::<Vec<_>>();
        assert_eq!(
            labels(kill_options(false, false, None, None)),
            ["Kill session", "Cancel"]
        );
        assert_eq!(
            labels(kill_options(true, false, None, None)),
            [
                "Kill only, keep worktree (restartable)",
                "Kill and delete worktree (not restartable)",
                "Cancel"
            ]
        );
        assert_eq!(
            labels(kill_options(true, true, None, None)),
            [
                "Kill only, keep worktree (restartable)",
                "Kill and delete worktree (not restartable)",
                "Kill, delete worktree and branch (not restartable)",
                "Cancel"
            ]
        );
        let blocked = inspection(false, &[], &["main worktree"]);
        assert_eq!(
            labels(kill_options(true, true, Some(&blocked), None)),
            ["Kill only, keep worktree (restartable)", "Cancel"]
        );
        // The branch's own blockers only take the branch deletion away.
        assert_eq!(
            labels(kill_options(true, true, None, Some(&blocked))),
            [
                "Kill only, keep worktree (restartable)",
                "Kill and delete worktree (not restartable)",
                "Cancel"
            ]
        );
        let kinds: Vec<KillOptionKind> = kill_options(true, true, None, None)
            .iter()
            .map(|option| option.kind)
            .collect();
        assert_eq!(
            kinds,
            [
                KillOptionKind::Kill,
                KillOptionKind::Delete,
                KillOptionKind::DeleteBranch,
                KillOptionKind::Cancel
            ]
        );
    }

    #[test]
    fn cleanup_summary_says_where_the_check_is() {
        assert_eq!(cleanup_summary(None), "Checking cleanup safety…");
        assert_eq!(
            cleanup_summary(Some(&inspection(false, &[], &["x"]))),
            "Worktree deletion is blocked:"
        );
        assert_eq!(
            cleanup_summary(Some(&inspection(true, &[], &[]))),
            "Local cleanup checks passed"
        );
        assert_eq!(
            cleanup_summary(Some(&inspection(false, &["a"], &[]))),
            "Deleting the worktree would lose (typing DELETE overrides):"
        );
        assert!(structural_blockers(None).is_empty());
    }

    #[test]
    fn merge_note_entries_put_the_worktree_note_first() {
        let mut parent = in_worktree(session("p", "Parent", SessionStatus::Running), "w1", "/wt", "feat");
        parent.notes = Some("  one\n\ntwo \n".to_owned());
        parent.shared_notes = Some(worktree_note("w1", "shared\n\n"));
        let mut child = session("c", "Child", SessionStatus::Running);
        child.parent_session_id = Some("p".to_owned());
        child.notes = Some("child note".to_owned());
        let mut quiet = session("q", "Quiet", SessionStatus::Running);
        quiet.parent_session_id = Some("p".to_owned());
        quiet.notes = Some("  \n ".to_owned());
        let all = [&parent, &child, &quiet];
        let entries = merge_note_entries(Some(&parent), &all);
        let shape: Vec<(&str, &str, Vec<&str>)> = entries
            .iter()
            .map(|entry| {
                (
                    entry.key.as_str(),
                    entry.title.as_str(),
                    entry.lines.iter().map(String::as_str).collect(),
                )
            })
            .collect();
        assert_eq!(
            shape,
            [
                ("worktree", "Worktree notes · feat", vec!["shared"]),
                ("p", "Parent", vec!["one", "two"]),
                ("c", "Child", vec!["child note"]),
            ]
        );
        assert!(merge_note_entries(None, &all).is_empty());
        // No branch: no suffix.
        let mut bare = parent.clone();
        bare.worktree = None;
        assert_eq!(merge_note_entries(Some(&bare), &[&bare])[0].title, "Worktree notes");
    }

    #[test]
    fn merge_tasks_are_the_worktrees_open_ones() {
        let tasks = deckhand_core::tasks::parse_tasks(
            "- [ ] open here <!-- dh:t=aaaa1111 wt=w1 -->\n- [x] done here <!-- dh:t=aaaa1112 wt=w1 -->\n- [ ] elsewhere <!-- dh:t=aaaa1113 wt=w2 -->\n- [ ] free\n",
        );
        let record = in_worktree(session("p", "P", SessionStatus::Running), "w1", "/wt", "feat");
        let titles: Vec<&str> = merge_tasks(&tasks, Some(&record))
            .iter()
            .map(|task| task.title.as_str())
            .collect();
        assert_eq!(titles, ["open here"]);
        assert!(merge_tasks(&tasks, None).is_empty());
        let loose = session("l", "L", SessionStatus::Running);
        assert!(merge_tasks(&tasks, Some(&loose)).is_empty());
    }

    #[test]
    fn merged_markers_come_from_the_worktree_or_the_session() {
        let mut record = in_worktree(session("p", "P", SessionStatus::Exited), "w1", "/wt", "feat");
        assert!(!has_merged_marker(Some(&record)));
        assert!(!has_merged_marker(None));
        assert_eq!(merged_target_branch(&record), None);
        record.merged_at = Some("2026-01-01T00:00:00.000Z".to_owned());
        record.merge_target_branch = Some("main".to_owned());
        assert!(has_merged_marker(Some(&record)));
        assert_eq!(merged_target_branch(&record), Some("main"));
        record.merged_at = None;
        if let Some(worktree) = record.worktree.as_mut() {
            worktree.merged_at = Some("2026-01-01T00:00:00.000Z".to_owned());
            worktree.merge_target_branch = Some("develop".to_owned());
        }
        assert!(has_merged_marker(Some(&record)));
        assert_eq!(merged_target_branch(&record), Some("develop"));
    }

    // ── Keys ─────────────────────────────────────────────────────────────────────────────────────────────────

    fn kill_app() -> (App, UnboundedReceiver<Update<App>>) {
        let (mut app, rx) = app_with(vec![in_worktree(
            session("s1", "beta", SessionStatus::Running),
            "w1",
            "/wt/beta",
            "beta",
        )]);
        app.set_mode(Mode::ConfirmKill);
        (app, rx)
    }

    #[test]
    fn kill_confirm_moves_wraps_and_cancels() {
        let (mut app, _rx) = kill_app();
        // Without an inspection: kill, delete, delete-branch, cancel.
        handle_confirm_kill(&mut app, &press("j"));
        assert_eq!(app.kill.confirm_index, 1);
        handle_confirm_kill(&mut app, &named(|key| key.down_arrow = true));
        handle_confirm_kill(&mut app, &press("j"));
        assert_eq!(app.kill.confirm_index, 3);
        handle_confirm_kill(&mut app, &press("j"));
        assert_eq!(app.kill.confirm_index, 0);
        handle_confirm_kill(&mut app, &press("k"));
        assert_eq!(app.kill.confirm_index, 3);
        handle_confirm_kill(&mut app, &named(|key| key.up_arrow = true));
        assert_eq!(app.kill.confirm_index, 2);
        assert!(!app.kill.keep_open_items);
        handle_confirm_kill(&mut app, &press(" "));
        assert!(app.kill.keep_open_items);
        handle_confirm_kill(&mut app, &press(" "));
        assert!(!app.kill.keep_open_items);
        handle_confirm_kill(&mut app, &press("z"));
        assert_eq!(app.mode, Mode::ConfirmKill);
        // Enter on Cancel closes it.
        app.kill.confirm_index = 3;
        handle_confirm_kill(&mut app, &enter());
        assert_eq!(app.mode, Mode::Browse);
        app.set_mode(Mode::ConfirmKill);
        handle_confirm_kill(&mut app, &escape());
        assert_eq!(app.mode, Mode::Browse);
    }

    #[test]
    fn the_kill_confirmation_shows_its_choices_once_both_checks_are_in() {
        let (mut app, _rx) = kill_app();
        let screen = |app: &mut App| -> (String, (u16, u16), (u16, u16)) {
            let size = app.browse.terminal_size;
            let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(size.cols, size.rows))
                .unwrap_or_else(|error| panic!("{error}"));
            terminal
                .draw(|frame| crate::app::layout::render_app(app, frame))
                .unwrap_or_else(|error| panic!("{error}"));
            let buffer = terminal.backend().buffer().clone();
            let left = app.layout().sidebar_width + 1;
            let cells: Vec<(u16, u16)> = (0..buffer.area.height)
                .flat_map(|y| (left..buffer.area.width).map(move |x| (x, y)))
                .collect();
            // The dialog's corners: inside the right pane's own.
            let top_left = cells
                .iter()
                .rev()
                .find(|&&(x, y)| buffer[(x, y)].symbol() == "╭")
                .copied();
            let bottom_right = cells.iter().find(|&&(x, y)| buffer[(x, y)].symbol() == "╯").copied();
            let text = (0..buffer.area.height)
                .map(|y| {
                    (0..buffer.area.width)
                        .map(|x| buffer[(x, y)].symbol())
                        .collect::<String>()
                })
                .collect::<Vec<_>>()
                .join("\n");
            (text, top_left.unwrap_or_default(), bottom_right.unwrap_or_default())
        };
        app.kill.cleanup_check = Some(CleanupCheck {
            session_id: "s1".to_owned(),
            worktree: None,
            branch: None,
        });
        // Checking: the question and Esc, no choices; keys other than Esc do nothing, Enter says why.
        assert!(kill_loading(&app));
        let (text, _, _) = screen(&mut app);
        assert!(
            text.contains("Checking cleanup safety…") && text.contains("esc cancel"),
            "{text}"
        );
        assert!(!text.contains("Kill only"), "{text}");
        handle_confirm_kill(&mut app, &press("j"));
        assert_eq!(app.kill.confirm_index, 0);
        handle_confirm_kill(&mut app, &enter());
        assert_eq!(app.mode, Mode::ConfirmKill);
        assert_eq!(
            app.messages.status_message.as_deref(),
            Some("Still checking cleanup safety…")
        );
        // The worktree's answer alone is not enough while the branch could go too.
        if let Some(check) = app.kill.cleanup_check.as_mut() {
            check.worktree = Some(inspection(false, &["1 untracked file"], &[]));
        }
        assert!(kill_loading(&app));
        if let Some(check) = app.kill.cleanup_check.as_mut() {
            check.branch = Some(inspection(
                false,
                &["1 untracked file", "3 unpublished commits", "the branch is not merged"],
                &[],
            ));
        }
        assert!(!kill_loading(&app));
        // Both in: the choices, and one size whichever check the selection shows.
        let (text, first_top, first_bottom) = screen(&mut app);
        assert!(text.contains("Kill only, keep worktree (restartable)"), "{text}");
        handle_confirm_kill(&mut app, &press("j"));
        handle_confirm_kill(&mut app, &press("j"));
        let (text, top, bottom) = screen(&mut app);
        assert!(text.contains("3 unpublished commits"), "{text}");
        assert_eq!((top, bottom), (first_top, first_bottom));
        // Esc works throughout.
        handle_confirm_kill(&mut app, &escape());
        assert_eq!(app.mode, Mode::Browse);
    }

    #[test]
    fn shift_j_and_k_scroll_a_dialog_that_does_not_fit_but_type_in_a_name() {
        // Macs have no page keys: J/K scroll (PgUp/PgDn too), the border says so.
        let (mut app, _rx) = app_with(vec![session("s1", "beta", SessionStatus::Exited)]);
        app.remove.items = (1..=60).map(|n| format!("item {n}")).collect();
        app.set_mode(Mode::ConfirmRemove);
        let draw = |app: &mut App| -> String {
            let size = app.browse.terminal_size;
            let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(size.cols, size.rows))
                .unwrap_or_else(|error| panic!("{error}"));
            terminal
                .draw(|frame| crate::app::layout::render_app(app, frame))
                .unwrap_or_else(|error| panic!("{error}"));
            let buffer = terminal.backend().buffer();
            (0..buffer.area.height)
                .map(|y| {
                    (0..buffer.area.width)
                        .map(|x| buffer[(x, y)].symbol())
                        .collect::<String>()
                })
                .collect::<Vec<_>>()
                .join("\n")
        };
        let text = draw(&mut app);
        assert!(text.contains("more · J") && !text.contains("item 60"), "{text}");
        app.handle_input(&press("J"));
        assert!(app.dialog.scroll > 0);
        let text = draw(&mut app);
        assert!(text.contains("more · K"), "{text}");
        app.handle_input(&press("K"));
        assert_eq!(app.dialog.scroll, 0);
        app.handle_input(&named(|key| key.page_down = true));
        assert!(app.dialog.scroll > 0);
        // In the name field J is a letter.
        app.set_mode(Mode::EnterName);
        app.create.draft_name = "a".to_owned();
        app.handle_input(&press("J"));
        assert_eq!(app.create.draft_name, "aJ");
    }

    #[test]
    fn kill_confirm_clamps_a_stale_index_to_the_options() {
        let (mut app, _rx) = kill_app();
        // A blocker removes the delete choices: [keep, cancel] while the index still says 3.
        app.kill.cleanup_check = Some(CleanupCheck {
            session_id: "s1".to_owned(),
            worktree: Some(inspection(false, &[], &["worktree is in use"])),
            branch: None,
        });
        app.kill.confirm_index = 3;
        handle_confirm_kill(&mut app, &press("k"));
        assert_eq!(app.kill.confirm_index, 0);
        app.kill.confirm_index = 9;
        handle_confirm_kill(&mut app, &enter());
        // Clamped to Cancel.
        assert_eq!(app.mode, Mode::Browse);
    }

    #[test]
    fn deleting_waits_for_the_inspection_then_follows_it() {
        let (mut app, _rx) = kill_app();
        app.kill.confirm_index = 1;
        // No inspection yet: nothing happens.
        handle_confirm_kill(&mut app, &enter());
        assert_eq!(app.mode, Mode::ConfirmKill);
        assert_eq!(app.messages.error, None);
        // An inspection of another session is never used.
        app.kill.cleanup_check = Some(CleanupCheck {
            session_id: "other".to_owned(),
            worktree: Some(inspection(true, &[], &[])),
            branch: None,
        });
        handle_confirm_kill(&mut app, &enter());
        assert_eq!(app.mode, Mode::ConfirmKill);
        // Not safe: the DELETE confirmation.
        app.kill.cleanup_check = Some(CleanupCheck {
            session_id: "s1".to_owned(),
            worktree: Some(inspection(false, &["1 untracked file(s)"], &[])),
            branch: Some(inspection(false, &["branch"], &[])),
        });
        app.kill.confirmation_draft = "stale".to_owned();
        app.review.details_scroll = 7;
        handle_confirm_kill(&mut app, &enter());
        assert_eq!(app.mode, Mode::ConfirmLoss);
        assert!(!app.kill.pending_delete_branch);
        assert_eq!(app.kill.confirmation_draft, "");
        assert_eq!(app.review.details_scroll, 0);
        // The branch deletion uses the branch inspection.
        app.set_mode(Mode::ConfirmKill);
        app.kill.confirm_index = 2;
        handle_confirm_kill(&mut app, &enter());
        assert_eq!(app.mode, Mode::ConfirmLoss);
        assert!(app.kill.pending_delete_branch);
    }

    #[test]
    fn typing_delete_edits_a_ten_character_draft() {
        let (mut app, _rx) = kill_app();
        app.set_mode(Mode::ConfirmLoss);
        for letter in ["D", "E", "L", "E", "T", "E"] {
            handle_confirm_loss(&mut app, &press(letter));
        }
        assert_eq!(app.kill.confirmation_draft, "DELETE");
        handle_confirm_loss(&mut app, &named(|key| key.backspace = true));
        assert_eq!(app.kill.confirmation_draft, "DELET");
        handle_confirm_loss(&mut app, &named(|key| key.delete = true));
        assert_eq!(app.kill.confirmation_draft, "DELE");
        // Control and meta chords are not text.
        handle_confirm_loss(
            &mut app,
            &Input::key(
                "a",
                Key {
                    ctrl: true,
                    ..Key::NONE
                },
            ),
        );
        handle_confirm_loss(
            &mut app,
            &Input::key(
                "b",
                Key {
                    meta: true,
                    ..Key::NONE
                },
            ),
        );
        assert_eq!(app.kill.confirmation_draft, "DELE");
        // Pasted text is sanitised and the draft stops at 10 UTF-16 units.
        handle_confirm_loss(&mut app, &press("0123456789"));
        assert_eq!(app.kill.confirmation_draft, "DELE012345");
        // Enter without a matching inspection does nothing (and without DELETE too).
        handle_confirm_loss(&mut app, &enter());
        assert_eq!(app.mode, Mode::ConfirmLoss);
        handle_confirm_loss(&mut app, &escape());
        assert_eq!(app.mode, Mode::Browse);
    }

    fn preview_json(uncommitted: i64) -> Value {
        json!({"sourceRef": "beta", "targetRoot": "/repo", "targetBranch": "main", "targetIsMain": true,
            "defaultBranch": "main", "commitCount": 0, "commits": [], "diff": {"files": 0, "insertions": 0,
            "deletions": 0}, "uncommitted": uncommitted, "overlap": {"committed": [], "uncommitted": []}})
    }

    #[test]
    fn the_merge_confirmation_cycles_through_its_rows() {
        let record = in_worktree(session("s1", "beta", SessionStatus::Running), "w1", "/wt/beta", "beta");
        let mut note_holder = session("n1", "Notes", SessionStatus::Running);
        note_holder.worktree = record.worktree.clone();
        note_holder.shared_notes = Some(worktree_note("w1", "- [ ] remember the tests\n"));
        let (mut app, _rx) = app_with(vec![record, note_holder]);
        app.set_mode(Mode::ConfirmMerge);
        app.merge.flow = Some(MergeFlow {
            session_id: "s1".to_owned(),
            commit_first: true,
            ..MergeFlow::default()
        });
        // Before the preview the choices are not shown: j/k and space do nothing.
        handle_confirm_merge(&mut app, &press("j"));
        handle_confirm_merge(&mut app, &press("k"));
        handle_confirm_merge(&mut app, &press("k"));
        assert_eq!(app.merge.confirm_index, 0);
        handle_confirm_merge(&mut app, &press(" "));
        assert_eq!(app.merge.flow.as_ref().map(|flow| flow.commit_first), Some(true));
        // With the preview: the items row joins the cycle, before the options.
        if let Some(flow) = app.merge.flow.as_mut() {
            flow.preview = serde_json::from_value(preview_json(2)).ok();
        }
        handle_confirm_merge(&mut app, &press(" "));
        assert_eq!(app.merge.flow.as_ref().map(|flow| flow.commit_first), Some(false));
        handle_confirm_merge(&mut app, &press("k"));
        assert_eq!(app.merge.flow.as_ref().map(|flow| flow.on_items), Some(true));
        // On the items row space and enter switch keeping the items.
        handle_confirm_merge(&mut app, &press(" "));
        assert_eq!(app.merge.flow.as_ref().map(|flow| flow.keep_open_items), Some(true));
        handle_confirm_merge(&mut app, &enter());
        assert_eq!(app.merge.flow.as_ref().map(|flow| flow.keep_open_items), Some(false));
        // j leaves the row for the first option; k from there comes back to it.
        handle_confirm_merge(&mut app, &press("j"));
        assert_eq!(app.merge.flow.as_ref().map(|flow| flow.on_items), Some(false));
        assert_eq!(app.merge.confirm_index, 0);
        handle_confirm_merge(&mut app, &press("k"));
        assert_eq!(app.merge.flow.as_ref().map(|flow| flow.on_items), Some(true));
        handle_confirm_merge(&mut app, &press("k"));
        assert_eq!(app.merge.flow.as_ref().map(|flow| flow.on_items), Some(false));
        assert_eq!(app.merge.confirm_index, 2);
        // Enter on Cancel; Esc anywhere.
        handle_confirm_merge(&mut app, &enter());
        assert_eq!(app.mode, Mode::Browse);
        app.set_mode(Mode::ConfirmMerge);
        handle_confirm_merge(&mut app, &escape());
        assert_eq!(app.mode, Mode::Browse);
    }

    #[test]
    fn enter_waits_for_the_merge_preview() {
        let (mut app, _rx) = app_with(vec![in_worktree(
            session("s1", "beta", SessionStatus::Running),
            "w1",
            "/wt/beta",
            "beta",
        )]);
        app.set_mode(Mode::ConfirmMerge);
        app.merge.flow = Some(MergeFlow {
            session_id: "s1".to_owned(),
            commit_first: true,
            ..MergeFlow::default()
        });
        handle_confirm_merge(&mut app, &enter());
        assert_eq!(
            app.messages.status_message.as_deref(),
            Some("Still checking what will be merged…")
        );
        assert_eq!(app.mode, Mode::ConfirmMerge);
        // Without it the choices are not shown, so Enter on Cancel does nothing either; Esc cancels.
        app.merge.confirm_index = 2;
        handle_confirm_merge(&mut app, &enter());
        assert_eq!(app.mode, Mode::ConfirmMerge);
        handle_confirm_merge(&mut app, &escape());
        assert_eq!(app.mode, Mode::Browse);
        // A failed preview lets the merge be tried (here without a daemon, so it does nothing more).
        app.set_mode(Mode::ConfirmMerge);
        app.set_status(None);
        app.merge.confirm_index = 0;
        if let Some(flow) = app.merge.flow.as_mut() {
            flow.preview_error = Some("boom".to_owned());
        }
        handle_confirm_merge(&mut app, &enter());
        assert_eq!(app.messages.status_message, None);
    }

    #[test]
    fn conflicts_and_removal_keys_need_a_daemon_and_ignore_others() {
        let (mut app, _rx) = app_with(vec![session("s1", "alpha", SessionStatus::Exited)]);
        app.set_mode(Mode::MergeConflicts);
        handle_merge_conflicts(&mut app, &press("q"));
        handle_merge_conflicts(&mut app, &enter());
        assert_eq!(app.mode, Mode::MergeConflicts);
        assert!(!app.messages.busy);
        app.set_mode(Mode::ConfirmRemove);
        handle_confirm_remove(&mut app, &press("x"));
        // x only removes when there are open items.
        assert_eq!(app.mode, Mode::ConfirmRemove);
        handle_confirm_remove(&mut app, &press("z"));
        assert_eq!(app.mode, Mode::ConfirmRemove);
        handle_confirm_remove(&mut app, &escape());
        assert_eq!(app.mode, Mode::Browse);
    }

    #[test]
    fn requesting_removal_lists_the_open_items_it_deletes() {
        let mut record = session("s1", "alpha", SessionStatus::Exited);
        record.notes = Some("- [ ] first\n- [x] done\n- [ ] second\n".to_owned());
        let (mut app, _rx) = app_with(vec![record]);
        request_remove(&mut app);
        assert_eq!(app.mode, Mode::ConfirmRemove);
        assert_eq!(app.remove.items, ["first", "second"]);
        // Only an exited session asks.
        let (mut running, _rx) = app_with(vec![session("s2", "beta", SessionStatus::Running)]);
        request_remove(&mut running);
        assert_eq!(running.mode, Mode::Browse);
    }

    // ── Daemon requests ──────────────────────────────────────────────────────────────────────────────────────

    #[test]
    fn opening_the_kill_confirmation_inspects_the_worktree_and_branch() {
        run(async {
            let (mut app, mut rx) = app_with(vec![in_worktree(
                session("s1", "beta", SessionStatus::Running),
                "w1",
                "/wt/beta",
                "beta",
            )]);
            let (client, requests) = fake_daemon(|request| {
                if request["deleteBranch"] == json!(true) {
                    json!({"ok": false, "error": "no branch info"})
                } else {
                    json!({"ok": true, "data": {"safe": false, "reasons": ["1 untracked file(s)"], "dirtyFiles": 0,
                        "untrackedFiles": 1, "ignoredFiles": 0, "structuralBlockers": []}})
                }
            });
            app.connection.client = Some(client);
            app.kill.confirm_index = 2;
            app.kill.force = true;
            app.kill.keep_open_items = true;
            open_kill_confirm(&mut app);
            assert_eq!(app.mode, Mode::ConfirmKill);
            assert_eq!(
                (app.kill.confirm_index, app.kill.force, app.kill.keep_open_items),
                (0, false, false)
            );
            assert_eq!(
                app.kill.cleanup_check.as_ref().map(|check| check.session_id.as_str()),
                Some("s1")
            );
            settle(&mut app, &mut rx).await;
            let asked: Vec<(Value, Value)> = requests
                .borrow()
                .iter()
                .map(|request| (request["type"].clone(), request["deleteBranch"].clone()))
                .collect();
            assert_eq!(
                asked,
                [
                    (json!("inspect-cleanup"), json!(false)),
                    (json!("inspect-cleanup"), json!(true))
                ]
            );
            let check = app.kill.cleanup_check.clone().unwrap();
            let worktree = check.worktree.unwrap();
            assert!(!worktree.inspection.safe);
            assert_eq!(worktree.inspection.reasons, ["1 untracked file(s)"]);
            // A failed inspection is an unsafe one that says why.
            let branch = check.branch.unwrap();
            assert!(!branch.inspection.safe);
            assert_eq!(branch.inspection.reasons, ["no branch info"]);
            assert!(branch.structural_blockers.is_empty());
        });
    }

    #[test]
    fn a_session_outside_a_worktree_has_nothing_to_inspect() {
        run(async {
            let (mut app, mut rx) = app_with(vec![session("s1", "beta", SessionStatus::Running)]);
            let (client, requests) = fake_daemon(|_| json!({"ok": true}));
            app.connection.client = Some(client);
            open_kill_confirm(&mut app);
            settle(&mut app, &mut rx).await;
            assert_eq!(app.mode, Mode::ConfirmKill);
            assert!(app.kill.cleanup_check.is_none());
            assert!(requests.borrow().is_empty());
            // Not a running session: nothing opens.
            let (mut exited, _rx) = app_with(vec![session("s2", "beta", SessionStatus::Exited)]);
            open_kill_confirm(&mut exited);
            assert_eq!(exited.mode, Mode::Browse);
        });
    }

    #[test]
    fn a_late_inspection_of_an_earlier_confirmation_is_dropped() {
        run(async {
            let (mut app, mut rx) = app_with(vec![in_worktree(
                session("s1", "beta", SessionStatus::Running),
                "w1",
                "/wt/beta",
                "beta",
            )]);
            let (client, _requests) = fake_daemon(|_| {
                json!({"ok": true, "data": {"safe": true, "reasons": [], "dirtyFiles": 0, "untrackedFiles": 0,
                    "ignoredFiles": 0, "structuralBlockers": []}})
            });
            app.connection.client = Some(client);
            open_kill_confirm(&mut app);
            // Reopened before the first answers arrive: only the latest request's answers count.
            open_kill_confirm(&mut app);
            assert_eq!(app.kill.cleanup_request, 2);
            app.kill.cleanup_request = 3;
            settle(&mut app, &mut rx).await;
            let check = app.kill.cleanup_check.clone().unwrap();
            assert!(check.worktree.is_none() && check.branch.is_none());
        });
    }

    #[test]
    fn x_upper_asks_with_the_force_kill_chosen_and_never_deletes() {
        run(async {
            let record = in_worktree(session("s1", "beta", SessionStatus::Running), "w1", "/wt/beta", "beta");
            let (mut app, mut rx) = app_with(vec![record]);
            let (client, requests) = fake_daemon(|_| json!({"ok": true}));
            app.connection.client = Some(client);
            crate::app::modes::browse::handle_browse(&mut app, &press("X"));
            // Nothing is checked or sent yet; the only choices are the force kill and Cancel.
            assert_eq!((app.mode, app.kill.force), (Mode::ConfirmKill, true));
            assert!(!kill_loading(&app) && requests.borrow().is_empty());
            let labels: Vec<&str> = current_kill_options(&app).iter().map(|option| option.label).collect();
            assert_eq!(labels, ["Force kill, keep worktree (restartable)", "Cancel"]);
            handle_confirm_kill(&mut app, &enter());
            settle(&mut app, &mut rx).await;
            let kill = requests.borrow()[0].clone();
            assert_eq!(kill["type"], "kill");
            assert_eq!(kill["force"], json!(true));
            assert_eq!(kill["deleteWorktree"], json!(false));
            assert_eq!(app.mode, Mode::Browse);
        });
    }

    #[test]
    fn killing_sends_the_choices_and_checks_it_stopped() {
        run(async {
            let mut record = in_worktree(session("s1", "beta", SessionStatus::Running), "w1", "/wt/beta", "beta");
            record.shared_notes = Some(worktree_note("w1", "- [ ] keep me\n"));
            let (mut app, mut rx) = app_with(vec![record]);
            let (client, requests) = fake_daemon(|_| json!({"ok": true}));
            app.connection.client = Some(client);
            app.set_mode(Mode::ConfirmKill);
            // The open items go to the backlog with a deletion unless kept...
            kill_selected(
                &mut app,
                KillOptions {
                    delete_branch: true,
                    ..KillOptions::default()
                },
            );
            assert!(app.messages.busy);
            settle(&mut app, &mut rx).await;
            let kill = requests.borrow()[0].clone();
            assert_eq!(kill["type"], "kill");
            assert_eq!(kill["deleteWorktree"], json!(true));
            assert_eq!(kill["deleteBranch"], json!(true));
            assert_eq!(kill["force"], json!(false));
            assert_eq!(kill["allowDataLoss"], json!(false));
            assert_eq!(kill["sendOpenItems"], json!(true));
            assert_eq!(app.mode, Mode::Browse);
            assert!(!app.messages.busy);
            // ...and a polite stop that did not end the session says so after 1.5 s.
            assert_eq!(app.messages.error, None);
            tokio::time::sleep(std::time::Duration::from_millis(1600)).await;
            while let Ok(update) = rx.try_recv() {
                update(&mut app);
            }
            assert_eq!(
                app.messages.error.as_deref(),
                Some("session still running; press X to force kill")
            );
            // Kept items, or no deletion: nothing is sent to the backlog; a force kill does not check.
            app.set_error(None);
            app.kill.keep_open_items = true;
            kill_selected(
                &mut app,
                KillOptions {
                    delete_worktree: true,
                    force: true,
                    allow_data_loss: true,
                    ..KillOptions::default()
                },
            );
            settle(&mut app, &mut rx).await;
            let kill = requests.borrow()[1].clone();
            assert_eq!(kill["deleteWorktree"], json!(true));
            assert_eq!(kill["deleteBranch"], json!(false));
            assert_eq!(kill["force"], json!(true));
            assert_eq!(kill["allowDataLoss"], json!(true));
            assert!(kill.get("sendOpenItems").is_none());
            tokio::time::sleep(std::time::Duration::from_millis(1600)).await;
            while let Ok(update) = rx.try_recv() {
                update(&mut app);
            }
            assert_eq!(app.messages.error, None);
        });
    }

    #[test]
    fn a_refused_kill_shows_the_error_and_stays() {
        run(async {
            let (mut app, mut rx) = app_with(vec![session("s1", "beta", SessionStatus::Running)]);
            let (client, _requests) = fake_daemon(|_| json!({"ok": false, "error": "cannot stop it"}));
            app.connection.client = Some(client);
            app.set_mode(Mode::ConfirmKill);
            handle_confirm_kill(&mut app, &enter());
            assert!(app.messages.busy);
            settle(&mut app, &mut rx).await;
            assert_eq!(app.messages.error.as_deref(), Some("cannot stop it"));
            assert_eq!(app.mode, Mode::ConfirmKill);
            assert!(!app.messages.busy);
        });
    }

    #[test]
    fn typing_delete_kills_with_the_data_loss_override() {
        run(async {
            let (mut app, mut rx) = app_with(vec![in_worktree(
                session("s1", "beta", SessionStatus::Running),
                "w1",
                "/wt/beta",
                "beta",
            )]);
            let (client, requests) = fake_daemon(|_| json!({"ok": true}));
            app.connection.client = Some(client);
            app.set_mode(Mode::ConfirmLoss);
            app.kill.cleanup_check = Some(CleanupCheck {
                session_id: "s1".to_owned(),
                worktree: Some(inspection(false, &["1 untracked file(s)"], &[])),
                branch: None,
            });
            app.kill.confirmation_draft = "DELET".to_owned();
            handle_confirm_loss(&mut app, &enter());
            assert!(requests.borrow().is_empty());
            app.kill.confirmation_draft = "DELETE".to_owned();
            handle_confirm_loss(&mut app, &enter());
            settle(&mut app, &mut rx).await;
            let kill = requests.borrow()[0].clone();
            assert_eq!(kill["deleteWorktree"], json!(true));
            assert_eq!(kill["allowDataLoss"], json!(true));
            assert_eq!(app.mode, Mode::Browse);
            // An inspection with blockers is not overridden.
            app.set_mode(Mode::ConfirmLoss);
            app.kill.confirmation_draft = "DELETE".to_owned();
            app.kill.cleanup_check = Some(CleanupCheck {
                session_id: "s1".to_owned(),
                worktree: Some(inspection(false, &[], &["main worktree"])),
                branch: None,
            });
            handle_confirm_loss(&mut app, &enter());
            settle(&mut app, &mut rx).await;
            assert_eq!(requests.borrow().len(), 1);
        });
    }

    #[test]
    fn opening_the_merge_confirmation_fetches_a_fresh_preview() {
        run(async {
            let (mut app, mut rx) = app_with(vec![in_worktree(
                session("s1", "beta", SessionStatus::Running),
                "w1",
                "/wt/beta",
                "beta",
            )]);
            let (client, requests) = fake_daemon(|_| json!({"ok": true, "data": preview_json(2)}));
            app.connection.client = Some(client);
            app.merge.confirm_index = 2;
            open_merge_confirm(&mut app);
            assert_eq!(app.mode, Mode::ConfirmMerge);
            assert_eq!(app.merge.confirm_index, 0);
            let flow = app.merge.flow.clone().unwrap();
            assert!(flow.commit_first && flow.preview.is_none());
            settle(&mut app, &mut rx).await;
            assert_eq!(requests.borrow()[0]["type"], "merge-preview");
            assert_eq!(requests.borrow()[0]["targetCwd"], "/repo");
            let flow = app.merge.flow.clone().unwrap();
            assert_eq!(flow.preview.map(|preview| preview.uncommitted), Some(2));
            // Not in a worktree: nothing opens.
            let (mut plain, _rx) = app_with(vec![session("s2", "plain", SessionStatus::Running)]);
            open_merge_confirm(&mut plain);
            assert_eq!(plain.mode, Mode::Browse);
        });
    }

    #[test]
    fn a_failed_or_stale_preview_is_kept_apart() {
        run(async {
            let (mut app, mut rx) = app_with(vec![in_worktree(
                session("s1", "beta", SessionStatus::Running),
                "w1",
                "/wt/beta",
                "beta",
            )]);
            let (client, _requests) = fake_daemon(|_| json!({"ok": false, "error": "not a git repository"}));
            app.connection.client = Some(client);
            open_merge_confirm(&mut app);
            settle(&mut app, &mut rx).await;
            let flow = app.merge.flow.clone().unwrap();
            assert_eq!(flow.preview_error.as_deref(), Some("not a git repository"));
            // Reopened meanwhile: the earlier answer is dropped.
            open_merge_confirm(&mut app);
            app.merge.request += 1;
            settle(&mut app, &mut rx).await;
            assert_eq!(app.merge.flow.clone().unwrap().preview_error, None);
        });
    }

    fn merge_result(extra: Value) -> Value {
        let mut result =
            json!({"mode": "merge", "sourceRef": "beta", "targetBranch": "main", "stdout": "", "stderr": ""});
        if let (Some(result), Some(extra)) = (result.as_object_mut(), extra.as_object()) {
            result.extend(extra.clone());
        }
        result
    }

    /// An app in the merge confirmation of a worktree session with a preview of `uncommitted` files.
    fn merging(uncommitted: i64, client: Rc<LiveClient>) -> (App, UnboundedReceiver<Update<App>>) {
        let (mut app, rx) = app_with(vec![in_worktree(
            session("s1", "beta", SessionStatus::Running),
            "w1",
            "/wt/beta",
            "beta",
        )]);
        app.connection.client = Some(client);
        app.set_mode(Mode::ConfirmMerge);
        app.merge.flow = Some(MergeFlow {
            session_id: "s1".to_owned(),
            commit_first: true,
            preview: serde_json::from_value(preview_json(uncommitted)).ok(),
            ..MergeFlow::default()
        });
        (app, rx)
    }

    #[test]
    fn merging_reports_what_was_applied() {
        run(async {
            let reply = Rc::new(RefCell::new(merge_result(json!({}))));
            let answer = Rc::clone(&reply);
            let (client, requests) = fake_daemon(move |_| json!({"ok": true, "data": answer.borrow().clone()}));
            let (mut app, mut rx) = merging(2, client);
            handle_confirm_merge(&mut app, &enter());
            assert!(app.messages.busy);
            settle(&mut app, &mut rx).await;
            let request = requests.borrow()[0].clone();
            assert_eq!(request["type"], "merge-worktree");
            assert_eq!(request["mode"], "merge");
            assert_eq!(request["commitFirst"], json!(true));
            assert!(request.get("sendOpenItems").is_none());
            assert_eq!(app.mode, Mode::Browse);
            assert_eq!(
                app.messages.status_message.as_deref(),
                Some("Merge applied without commit from beta into main")
            );
            // Squash, after committing, with open items sent.
            *reply.borrow_mut() =
                merge_result(json!({"mode": "squash", "committed": {"files": 1, "sha": "abc"}, "sentItems": 2}));
            app.set_mode(Mode::ConfirmMerge);
            app.merge.confirm_index = 1;
            handle_confirm_merge(&mut app, &enter());
            settle(&mut app, &mut rx).await;
            assert_eq!(requests.borrow()[1]["mode"], "squash");
            assert_eq!(
                app.messages.status_message.as_deref(),
                Some(
                    "Committed 1 file, then squash applied from beta into main · 2 open note items sent to the backlog"
                )
            );
            // Nothing to merge.
            *reply.borrow_mut() = merge_result(json!({"skipped": true}));
            app.set_mode(Mode::ConfirmMerge);
            app.merge.confirm_index = 0;
            handle_confirm_merge(&mut app, &enter());
            settle(&mut app, &mut rx).await;
            assert_eq!(
                app.messages.status_message.as_deref(),
                Some("Skipped merge: no new commits from beta into main")
            );
        });
    }

    #[test]
    fn commit_first_applies_only_with_uncommitted_files_and_the_toggle() {
        run(async {
            let (client, requests) = fake_daemon(|_| json!({"ok": true, "data": merge_result(json!({}))}));
            let (mut app, mut rx) = merging(0, client);
            merge_selected(&mut app, WorktreeMergeMode::Merge);
            settle(&mut app, &mut rx).await;
            assert_eq!(requests.borrow()[0]["commitFirst"], json!(false));
            // Toggled off with files to commit.
            if let Some(flow) = app.merge.flow.as_mut() {
                flow.preview = serde_json::from_value(preview_json(3)).ok();
                flow.commit_first = false;
            }
            merge_selected(&mut app, WorktreeMergeMode::Merge);
            settle(&mut app, &mut rx).await;
            assert_eq!(requests.borrow()[1]["commitFirst"], json!(false));
            // On, with files to commit.
            if let Some(flow) = app.merge.flow.as_mut() {
                flow.commit_first = true;
            }
            merge_selected(&mut app, WorktreeMergeMode::Merge);
            settle(&mut app, &mut rx).await;
            assert_eq!(requests.borrow()[2]["commitFirst"], json!(true));
            // Without a flow of this session there is no preview, so nothing to commit.
            app.merge.flow = None;
            merge_selected(&mut app, WorktreeMergeMode::Merge);
            settle(&mut app, &mut rx).await;
            assert_eq!(requests.borrow()[3]["commitFirst"], json!(false));
        });
    }

    #[test]
    fn a_failed_merge_is_shown_on_the_confirmation() {
        run(async {
            let (client, _requests) = fake_daemon(|_| json!({"ok": false, "error": "hook failed\nline two"}));
            let (mut app, mut rx) = merging(2, client);
            app.set_error(Some("old".to_owned()));
            handle_confirm_merge(&mut app, &enter());
            assert_eq!(app.messages.error, None);
            settle(&mut app, &mut rx).await;
            assert_eq!(app.mode, Mode::ConfirmMerge);
            assert_eq!(app.messages.error, None);
            assert_eq!(
                app.merge.flow.as_ref().and_then(|flow| flow.error.clone()).as_deref(),
                Some("hook failed\nline two")
            );
            assert!(!app.messages.busy);
        });
    }

    #[test]
    fn a_conflicted_merge_is_kept_or_aborted() {
        run(async {
            let result = merge_result(json!({"conflicted": true, "conflicts": ["a.txt", "b.txt"], "conflictCount": 2}));
            let updated = serde_json::to_value(session("s1", "beta (merged)", SessionStatus::Running)).unwrap();
            let (client, requests) = fake_daemon({
                let result = result.clone();
                let updated = updated.clone();
                move |request| {
                    if request["type"] == "merge-worktree" {
                        json!({"ok": true, "data": result})
                    } else {
                        json!({"ok": true, "data": updated})
                    }
                }
            });
            let (mut app, mut rx) = merging(2, client);
            handle_confirm_merge(&mut app, &enter());
            settle(&mut app, &mut rx).await;
            assert_eq!(app.mode, Mode::MergeConflicts);
            assert_eq!(
                app.merge.conflict.as_ref().map(|conflict| conflict.session_id.as_str()),
                Some("s1")
            );
            assert!(!app.messages.busy);
            // Esc keeps it.
            handle_merge_conflicts(&mut app, &escape());
            settle(&mut app, &mut rx).await;
            assert_eq!(requests.borrow()[1]["type"], "resolve-merge");
            assert_eq!(requests.borrow()[1]["action"], "keep");
            assert_eq!(requests.borrow()[1]["targetCwd"], "/repo");
            assert_eq!(app.mode, Mode::Browse);
            assert!(app.merge.conflict.is_none());
            assert_eq!(
                app.messages.status_message.as_deref(),
                Some("Merge kept with conflicts in 2 files into main: resolve them, then commit (marked merged)")
            );
            assert_eq!(app.data.repo_sessions[0].title, "beta (merged)");
            // x aborts.
            app.set_mode(Mode::MergeConflicts);
            app.merge.conflict = Some(MergeConflict {
                session_id: "s1".to_owned(),
                result: serde_json::from_value(result).unwrap(),
            });
            handle_merge_conflicts(&mut app, &press("x"));
            settle(&mut app, &mut rx).await;
            assert_eq!(requests.borrow()[2]["action"], "abort");
            assert_eq!(
                app.messages.status_message.as_deref(),
                Some("Merge aborted; main is as it was")
            );
            assert_eq!(app.mode, Mode::Browse);
        });
    }

    #[test]
    fn the_conflict_count_falls_back_to_the_listed_files() {
        run(async {
            let updated = serde_json::to_value(session("s1", "beta", SessionStatus::Running)).unwrap();
            let (client, _requests) = fake_daemon(move |_| json!({"ok": true, "data": updated}));
            let (mut app, mut rx) = merging(0, client);
            app.merge.conflict = Some(MergeConflict {
                session_id: "s1".to_owned(),
                result: serde_json::from_value(merge_result(json!({"conflicted": true, "conflicts": ["a.txt"]})))
                    .unwrap(),
            });
            app.set_mode(Mode::MergeConflicts);
            resolve_conflicted_merge(&mut app, ResolveMergeAction::Keep);
            settle(&mut app, &mut rx).await;
            assert_eq!(
                app.messages.status_message.as_deref(),
                Some("Merge kept with conflicts in 1 file into main: resolve them, then commit (marked merged)")
            );
        });
    }

    #[test]
    fn a_refused_resolution_keeps_the_conflict_screen() {
        run(async {
            let (client, _requests) = fake_daemon(|_| json!({"ok": false, "error": "no merge in progress"}));
            let (mut app, mut rx) = merging(0, client);
            app.merge.conflict = Some(MergeConflict {
                session_id: "s1".to_owned(),
                result: serde_json::from_value(merge_result(json!({"conflicted": true}))).unwrap(),
            });
            app.set_mode(Mode::MergeConflicts);
            handle_merge_conflicts(&mut app, &press("x"));
            settle(&mut app, &mut rx).await;
            assert_eq!(app.messages.error.as_deref(), Some("no merge in progress"));
            assert_eq!(app.mode, Mode::MergeConflicts);
            assert!(app.merge.conflict.is_some());
        });
    }

    #[test]
    fn removing_an_archived_session_moves_its_open_items_first() {
        run(async {
            let mut record = session("s1", "alpha", SessionStatus::Exited);
            record.notes = Some("- [ ] first\n".to_owned());
            let (mut app, mut rx) = app_with(vec![record]);
            let (client, requests) = fake_daemon(|_| json!({"ok": true}));
            app.connection.client = Some(client);
            app.panes.preview.content = "old".to_owned();
            request_remove(&mut app);
            handle_confirm_remove(&mut app, &enter());
            assert_eq!(app.mode, Mode::Browse);
            assert!(app.messages.busy);
            settle(&mut app, &mut rx).await;
            let request = requests.borrow()[0].clone();
            assert_eq!(request["type"], "remove");
            assert_eq!(request["moveOpenItems"], json!(true));
            assert_eq!(
                app.messages.status_message.as_deref(),
                Some("Moved its open note items to Tasks (b)")
            );
            assert_eq!(app.panes.preview.content, "");
            // x removes them with the notes.
            app.set_status(None);
            request_remove(&mut app);
            handle_confirm_remove(&mut app, &press("x"));
            settle(&mut app, &mut rx).await;
            assert!(requests.borrow()[1].get("moveOpenItems").is_none());
            assert_eq!(app.messages.status_message, None);
            // Without open items Enter just removes.
            app.data.repo_sessions[0].notes = None;
            request_remove(&mut app);
            handle_confirm_remove(&mut app, &enter());
            settle(&mut app, &mut rx).await;
            assert!(requests.borrow()[2].get("moveOpenItems").is_none());
        });
    }

    #[test]
    fn a_refused_removal_shows_the_error() {
        run(async {
            let (mut app, mut rx) = app_with(vec![session("s1", "alpha", SessionStatus::Exited)]);
            let (client, _requests) = fake_daemon(|_| json!({"ok": false, "error": "it is still attached"}));
            app.connection.client = Some(client);
            remove_selected(&mut app, false);
            settle(&mut app, &mut rx).await;
            assert_eq!(app.messages.error.as_deref(), Some("it is still attached"));
            // Only an exited session can be removed.
            let (mut running, _rx) = app_with(vec![session("s2", "beta", SessionStatus::Running)]);
            running.connection.client = app.connection.client.clone();
            remove_selected(&mut running, false);
            assert!(!running.messages.busy);
        });
    }

    // ── Pane snapshots (fixtures/ui/panes) ─────────────────────────────────────────────────────────────────────

    /// The rows of a pane drawn at an explicit 98×31 size, right-trimmed.
    fn drawn(draw: impl FnOnce(&mut Buffer, Rect)) -> Vec<String> {
        let area = Rect::new(0, 0, 98, 31);
        let mut buf = Buffer::empty(area);
        draw(&mut buf, area);
        (0..area.height)
            .map(|y| {
                let mut row = String::new();
                let mut x = 0;
                while x < area.width {
                    let symbol = buf[(x, y)].symbol();
                    row.push_str(symbol);
                    x += u16::try_from(string_width(symbol).max(1)).unwrap_or(1);
                }
                row.trim_end().to_owned()
            })
            .collect()
    }

    /// The pane drawn by `draw` matches the snapshot `fixtures/ui/panes/<name>.txt`.
    fn assert_pane(name: &str, draw: impl FnOnce(&mut Buffer, Rect)) {
        deckhand_testkit::assert_snapshot(format!("fixtures/ui/panes/{name}.txt"), &drawn(draw).join("\n"));
    }

    fn refs(sessions: &[SessionRecord]) -> Vec<&SessionRecord> {
        sessions.iter().collect()
    }

    fn untracked_draft() -> SessionCleanupInspection {
        let mut inspection = inspection(false, &["1 untracked file(s)"], &[]);
        inspection.inspection.untracked_files = 1;
        inspection.inspection.files = serde_json::from_value(json!([{"path": "draft.txt", "state": "untracked"}])).ok();
        inspection
    }

    #[test]
    fn the_kill_confirmation_matches_the_snapshot() {
        let eps = in_worktree(
            session("s5", "eps", SessionStatus::Running),
            "w5",
            "<TMP>/home/worktrees/eps",
            "eps",
        );
        let sessions = [eps];
        let inspected = untracked_draft();
        let options = kill_options(true, true, Some(&inspected), Some(&inspected));
        let props = KillConfirmPaneProps {
            session: sessions.first(),
            sessions: &refs(&sessions),
            options: &options,
            selected_index: 0,
            force: false,
            inspection: Some(&inspected),
            open_items: &[],
            keep_open_items: false,
            loading: false,
            other_inspection: None,
        };
        assert_pane("47-confirm-kill", |buf, area| {
            render_kill_confirm_pane(&props, buf, area);
        });
    }

    #[test]
    fn the_blocked_kill_confirmation_matches_the_snapshot() {
        let beta = in_worktree(
            session("s2", "beta", SessionStatus::Running),
            "w2",
            "<TMP>/home/worktrees/beta",
            "beta",
        );
        let sessions = [beta];
        let blocked = inspection(false, &[], &["worktree is in use by session \"beta / gamma\""]);
        let options = kill_options(true, true, Some(&blocked), None);
        let props = KillConfirmPaneProps {
            session: sessions.first(),
            sessions: &refs(&sessions),
            options: &options,
            selected_index: 0,
            force: false,
            inspection: Some(&blocked),
            open_items: &[],
            keep_open_items: false,
            loading: false,
            other_inspection: None,
        };
        assert_pane("46-confirm-kill-blocked", |buf, area| {
            render_kill_confirm_pane(&props, buf, area);
        });
    }

    #[test]
    fn the_kill_confirmation_lists_the_open_items() {
        let beta = in_worktree(session("s2", "beta", SessionStatus::Running), "w2", "/wt/beta", "beta");
        let sessions = [beta];
        let options = kill_options(true, false, None, None);
        let items = vec!["write docs".to_owned(), "fix bug".to_owned()];
        let mut props = KillConfirmPaneProps {
            session: sessions.first(),
            sessions: &refs(&sessions),
            options: &options,
            selected_index: 1,
            force: false,
            inspection: None,
            open_items: &items,
            keep_open_items: false,
            loading: false,
            other_inspection: None,
        };
        let rows = dialog_rows(&drawn(|buf, area| render_kill_confirm_pane(&props, buf, area)));
        let text: Vec<&str> = rows
            .iter()
            .skip(2)
            .map(|row| row.trim_matches(|c| c == '│' || c == ' '))
            .collect();
        // The question and the path on top, what deleting would lose below them, the choices at the bottom.
        assert_eq!(text[0], "Kill \"beta\"?");
        assert_eq!(text[1], "/wt/beta");
        assert_eq!(text[3], "Checking cleanup safety…");
        let at = |text: &[&str], start: &str| {
            text.iter()
                .position(|row| row.starts_with(start))
                .unwrap_or_else(|| panic!("{start}: {text:#?}"))
        };
        let kill_only = at(&text, "Kill only, keep worktree (restartable)");
        assert!(kill_only > 3);
        assert!(text[kill_only + 1].starts_with("❯ Kill and delete worktree (not restartable)"));
        assert_eq!(
            text[kill_only + 3],
            "☑ If deleted: 2 open note items → backlog: write docs, fix bug"
        );
        // The hint sits at the dialog's bottom: above the margin row and the border.
        assert_eq!(text[text.len() - 3], "enter choose • space note items • esc cancel");
        props.keep_open_items = true;
        let rows = dialog_rows(&drawn(|buf, area| render_kill_confirm_pane(&props, buf, area)));
        assert!(rows.iter().any(|row| {
            row.contains("☐ If deleted: 2 open note items stay in its (read-only) note: write docs, fix bug")
        }));
        // Without a session or a deletion choice: the plain question, no open-items row.
        let only_kill = kill_options(false, false, None, None);
        let bare = KillConfirmPaneProps {
            session: None,
            sessions: &[],
            options: &only_kill,
            selected_index: 0,
            force: false,
            inspection: None,
            open_items: &items,
            keep_open_items: false,
            loading: false,
            other_inspection: None,
        };
        let rows = dialog_rows(&drawn(|buf, area| render_kill_confirm_pane(&bare, buf, area)));
        assert!(rows[2].contains("Kill session?"));
        assert!(!rows.iter().any(|row| row.contains("If deleted")));
    }

    fn feat_preview() -> MergePreview {
        serde_json::from_value(
            json!({"sourceRef": "feat", "targetRoot": "<TMP>/demo", "targetBranch": "main",
            "targetIsMain": true, "defaultBranch": "main", "commitCount": 1, "commits": ["feat change"],
            "diff": {"files": 1, "insertions": 1, "deletions": 1}, "uncommitted": 0,
            "overlap": {"committed": [], "uncommitted": []}}),
        )
        .unwrap()
    }

    #[test]
    fn the_merge_confirmation_matches_the_snapshots() {
        let mut beta = in_worktree(
            session("s2", "beta", SessionStatus::Running),
            "w2",
            "<TMP>/home/worktrees/beta",
            "beta",
        );
        beta.shared_notes = Some(worktree_note("w2", "Remember the tests\n"));
        let sessions = [beta];
        let preview: MergePreview = serde_json::from_value(json!({"sourceRef": "beta", "targetRoot": "<TMP>/demo",
            "targetBranch": "main", "targetIsMain": true, "defaultBranch": "main", "commitCount": 0, "commits": [],
            "diff": {"files": 0, "insertions": 0, "deletions": 0}, "uncommitted": 2,
            "overlap": {"committed": [], "uncommitted": []}}))
        .unwrap();
        let flow = MergeFlow {
            session_id: "s2".to_owned(),
            preview: Some(preview),
            commit_first: true,
            ..MergeFlow::default()
        };
        let props = MergeConfirmPaneProps {
            session: sessions.first(),
            sessions: &refs(&sessions),
            flow: Some(&flow),
            selected_index: 0,
            tasks: &[],
            now: None,
        };
        assert_pane("45-confirm-merge", |buf, area| {
            render_merge_confirm_pane(&props, buf, area);
        });

        let feat = in_worktree(
            session("s1", "feat", SessionStatus::Running),
            "w1",
            "<TMP>/home/worktrees/feat",
            "feat",
        );
        let sessions = [feat];
        let flow = MergeFlow {
            session_id: "s1".to_owned(),
            preview: Some(feat_preview()),
            commit_first: true,
            ..MergeFlow::default()
        };
        let props = MergeConfirmPaneProps {
            session: sessions.first(),
            sessions: &refs(&sessions),
            flow: Some(&flow),
            selected_index: 0,
            tasks: &[],
            now: None,
        };
        assert_pane("58-confirm-merge-commits", |buf, area| {
            render_merge_confirm_pane(&props, buf, area);
        });
    }

    #[test]
    fn the_merge_confirmation_marks_the_selected_row_and_the_waiting_state() {
        let feat = in_worktree(session("s1", "feat", SessionStatus::Running), "w1", "/wt/feat", "feat");
        let sessions = [feat];
        let waiting = MergeConfirmPaneProps {
            session: sessions.first(),
            sessions: &refs(&sessions),
            flow: None,
            selected_index: 2,
            tasks: &[],
            now: None,
        };
        let rows = dialog_rows(&drawn(|buf, area| render_merge_confirm_pane(&waiting, buf, area)));
        // A full-pane screen: the title right under the border.
        assert!(rows[1].contains("Merge \"feat\"?"));
        assert!(rows[2].contains("Checking what will be merged…"));
        // While it checks, no choices (they would move down when the preview arrives): only Esc.
        assert!(
            !rows
                .iter()
                .any(|row| row.contains("Cancel") || row.contains("Merge into"))
        );
        assert!(rows.iter().any(|row| row.contains("esc cancel")));
        // A failed attempt's output shows in full, above the choices.
        let flow = MergeFlow {
            session_id: "s1".to_owned(),
            preview: Some(feat_preview()),
            commit_first: true,
            error: Some("hook failed".to_owned()),
            ..MergeFlow::default()
        };
        let failed = MergeConfirmPaneProps {
            flow: Some(&flow),
            selected_index: 0,
            ..waiting
        };
        let rows = dialog_rows(&drawn(|buf, area| render_merge_confirm_pane(&failed, buf, area)));
        assert!(rows.iter().any(|row| row.contains("hook failed")));
    }

    #[test]
    fn the_conflict_screen_matches_the_snapshot() {
        let result: WorktreeMergeResult = serde_json::from_value(merge_result(
            json!({"conflicted": true, "conflicts": ["file.txt"], "conflictCount": 1}),
        ))
        .unwrap();
        assert_pane("59-merge-conflicts", |buf, area| {
            render_merge_conflict_pane(&result, buf, area);
        });
    }

    #[test]
    fn the_remove_confirmation_matches_the_snapshot() {
        let alpha = session("s1", "alpha", SessionStatus::Exited);
        let props = RemoveConfirmPaneProps {
            session: Some(&alpha),
            items: &[],
        };
        assert_pane("55-confirm-remove", |buf, area| {
            render_remove_confirm_pane(&props, buf, area);
        });
    }

    #[test]
    fn the_remove_confirmation_lists_every_open_item() {
        let items: Vec<String> = (1..=10).map(|n| format!("item {n}")).collect();
        let props = RemoveConfirmPaneProps {
            session: None,
            items: &items,
        };
        let rows = dialog_rows(&drawn(|buf, area| render_remove_confirm_pane(&props, buf, area)));
        let text: Vec<&str> = rows
            .iter()
            .skip(2)
            .map(|row| row.trim_matches(|c| c == '│' || c == ' '))
            .collect();
        assert_eq!(text[0], "Remove “session” for good?");
        assert_eq!(text[2], "It cannot be resumed afterwards; its notes go to notes/trash.");
        assert_eq!(text[3], "Its notes have 10 unchecked items:");
        assert_eq!(text[4], "☐ item 1");
        assert_eq!(text[13], "☐ item 10");
        assert!(!rows.iter().any(|row| row.contains("more")));
        // The hint sits at the dialog's bottom: above the margin row and the border.
        assert_eq!(
            text[text.len() - 3],
            "enter move them to Tasks, then remove • x remove anyway • esc cancel"
        );
        let one = [String::from("only")];
        let props = RemoveConfirmPaneProps {
            session: None,
            items: &one,
        };
        let rows = dialog_rows(&drawn(|buf, area| render_remove_confirm_pane(&props, buf, area)));
        assert!(rows[5].contains("1 unchecked item:"));
    }

    /// `compactPath` reads `$HOME`: tests that set it hold this lock (TS `withEnv`).
    static HOME_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// The rows of a pane drawn by `draw` into `width` x `height`, as plain text.
    fn plain_rows(width: u16, height: u16, draw: impl FnOnce(&mut Buffer, Rect)) -> Vec<String> {
        let area = Rect::new(0, 0, width, height);
        let mut buf = Buffer::empty(area);
        draw(&mut buf, area);
        (0..height)
            .map(|y| {
                let mut row = String::new();
                let mut x = 0;
                while x < width {
                    let symbol = buf[(x, y)].symbol();
                    row.push_str(symbol);
                    x += u16::try_from(string_width(symbol).max(1)).unwrap_or(1);
                }
                row
            })
            .collect()
    }

    #[test]
    fn rendered_confirmation_fits_its_pane_and_the_conflict_view_shows_the_choices() {
        let _guard = HOME_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let saved = std::env::var_os("HOME");
        // SAFETY: every test of this module that reads or writes HOME holds HOME_LOCK.
        unsafe { std::env::set_var("HOME", "/home/me") };
        let outcome = std::panic::catch_unwind(|| {
            let record: SessionRecord = serde_json::from_value(json!({
                "id": "s", "title": "auth refactor", "program": "claude", "command": "claude",
                "cwd": "/home/me/wt/auth", "repoRoot": "/home/me/code/app",
                "worktree": {"mode": "managed", "id": "w", "path": "/home/me/wt/auth", "branch": "auth"},
                "status": "running", "createdAt": "", "updatedAt": "", "notes": "- [ ] cookie flags"
            }))
            .unwrap();
            let preview: MergePreview = serde_json::from_value(json!({
                "sourceRef": "feat/auth", "sourceSha": "a".repeat(40), "targetRoot": "/home/me/wt/release",
                "targetBranch": "release", "targetIsMain": false, "defaultBranch": "main", "commitCount": 8,
                "commits": ["add login form", "wire the session cookie", "fix tests", "rename handler", "docs", "lint"],
                "diff": {"files": 5, "insertions": 120, "deletions": 14}, "uncommitted": 4,
                "overlap": {"committed": ["src/auth.ts"], "uncommitted": []}
            }))
            .unwrap();
            let flow = MergeFlow {
                session_id: "s".to_owned(),
                commit_first: true,
                preview: Some(preview),
                ..MergeFlow::default()
            };
            let sessions = [record.clone()];
            // In a 48-row pane the notes show; in a 16-row one they are further down the body (it scrolls).
            for height in [48u16, 16] {
                let props = MergeConfirmPaneProps {
                    session: sessions.first(),
                    sessions: &refs(&sessions),
                    flow: Some(&flow),
                    selected_index: 1,
                    tasks: &[],
                    now: None,
                };
                // The full 92-column pane.
                let lines = dialog_rows(&plain_rows(92, height, |buf, area| {
                    render_merge_confirm_pane(&props, buf, area)
                }));
                assert!(lines.len() <= usize::from(height), "{height}");
                assert!(
                    lines.iter().any(|line| line.contains("Into release · ~/wt/release")),
                    "{lines:#?}"
                );
                assert!(
                    lines
                        .iter()
                        .any(|line| line.contains("☑ commit them first (\"auth refactor\")")),
                    "{lines:#?}"
                );
                assert!(lines.iter().any(|line| {
                    line.contains("⚠ Target has uncommitted changes in 1 file the merge touches: src/auth.ts")
                }));
                assert!(
                    lines
                        .iter()
                        .any(|line| line.contains("❯ Squash merge into release without committing"))
                );
                assert_eq!(
                    lines.iter().any(|line| line.contains("cookie flags")),
                    height == 48,
                    "{height}"
                );
            }
            // The worktree's task is ticked by the merge; its note's open items are one row, here selected.
            let tasks = deckhand_core::tasks::parse_tasks(
                "- [ ] Add OAuth <!-- dh:t=a wt=w -->\n- [ ] Elsewhere <!-- dh:t=c wt=x -->",
            );
            let mut with_note = record.clone();
            with_note.shared_notes = Some(SharedNote {
                path: "/n/w.md".to_owned(),
                revision: "r".to_owned(),
                ..worktree_note("w", "- [x] Read\n- [ ] Add tests")
            });
            let sessions = [with_note];
            let on_items = MergeFlow {
                on_items: true,
                ..flow.clone()
            };
            let props = MergeConfirmPaneProps {
                session: sessions.first(),
                sessions: &refs(&sessions),
                flow: Some(&on_items),
                selected_index: 0,
                tasks: &tasks,
                now: None,
            };
            let with_tasks = dialog_rows(&plain_rows(92, 30, |buf, area| {
                render_merge_confirm_pane(&props, buf, area)
            }));
            assert!(
                with_tasks
                    .iter()
                    .any(|line| line.contains("◆ Task \"Add OAuth\" will be marked done"))
            );
            assert!(
                with_tasks
                    .iter()
                    .any(|line| line.contains("❯ ☑ 1 open note item → backlog: Add tests"))
            );
            assert!(!with_tasks.iter().any(|line| line.contains("Elsewhere")));
            assert!(
                !with_tasks.iter().any(|line| line.contains("❯ Merge into")),
                "the options are not selected while the items row is"
            );
            let result: WorktreeMergeResult = serde_json::from_value(merge_result(
                json!({"sourceRef": "auth", "conflicted": true, "conflicts": ["src/auth.ts"], "conflictCount": 1}),
            ))
            .unwrap();
            // The full 80-column pane.
            let conflict = dialog_rows(&plain_rows(80, 7, |buf, area| {
                render_merge_conflict_pane(&result, buf, area)
            }));
            // Seven rows leave no room for the blank margin rows.
            let inner: Vec<String> = conflict[1..conflict.len() - 1]
                .iter()
                .map(|line| line.chars().skip(2).collect::<String>())
                .map(|line| line.trim_end_matches(['│', ' ']).to_owned())
                .collect();
            assert_eq!(
                inner,
                [
                    "Merged with conflicts in 1 file",
                    "  src/auth.ts",
                    "",
                    "enter keep it: resolve it in your editor or the Git tab (esc too)",
                    "x     abort the merge"
                ]
            );
        });
        match saved {
            // SAFETY: as above.
            Some(saved) => unsafe { std::env::set_var("HOME", saved) },
            // SAFETY: as above.
            None => unsafe { std::env::remove_var("HOME") },
        }
        if let Err(panic) = outcome {
            std::panic::resume_unwind(panic);
        }
    }

    #[test]
    fn the_panes_do_not_panic_in_tiny_areas() {
        let sessions = [in_worktree(
            session("s1", "feat", SessionStatus::Running),
            "w1",
            "/wt",
            "feat",
        )];
        let result: WorktreeMergeResult = serde_json::from_value(merge_result(json!({"conflicted": true}))).unwrap();
        let options = kill_options(true, true, None, None);
        let items = vec!["a".to_owned()];
        for (width, height) in [(0, 0), (1, 1), (3, 2), (5, 3), (12, 6)] {
            let area = Rect::new(0, 0, width, height);
            let mut buf = Buffer::empty(area);
            render_kill_confirm_pane(
                &KillConfirmPaneProps {
                    session: sessions.first(),
                    sessions: &refs(&sessions),
                    options: &options,
                    selected_index: 0,
                    force: false,
                    inspection: None,
                    open_items: &items,
                    keep_open_items: false,
                    loading: false,
                    other_inspection: None,
                },
                &mut buf,
                area,
            );
            render_merge_confirm_pane(
                &MergeConfirmPaneProps {
                    session: sessions.first(),
                    sessions: &refs(&sessions),
                    flow: None,
                    selected_index: 0,
                    tasks: &[],
                    now: None,
                },
                &mut buf,
                area,
            );
            render_merge_conflict_pane(&result, &mut buf, area);
            render_remove_confirm_pane(
                &RemoveConfirmPaneProps {
                    session: sessions.first(),
                    items: &items,
                },
                &mut buf,
                area,
            );
        }
    }

    #[test]
    fn the_keymaps_name_their_keys() {
        for keymap in [KILL_KEYMAP, MERGE_KEYMAP, REMOVE_KEYMAP] {
            assert!(!keymap.is_empty());
            assert!(crate::keymap::unknown_topics(keymap).is_empty());
        }
        assert!(MERGE_KEYMAP.iter().any(|binding| binding.matches(&press("x"))));
        assert!(KILL_KEYMAP.iter().any(|binding| binding.matches(&escape())));
    }
}
