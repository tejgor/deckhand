//! W, the worktree manager. Every worktree of the repository (Git's list plus Deckhand's records), grouped by what to
//! do with it, with its sessions, merge state and what deleting it would lose. x deletes one (or, on the ready group's
//! heading, all of them) after a confirmation that names the sessions it stops; data loss still needs a typed DELETE.
//! The daemon re-checks everything at deletion time.

use std::cell::RefCell;
use std::rc::Rc;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use deckhand_client::live::{DeleteWorktreeOptions, LiveClient};
use deckhand_core::display::{
    HINT_SEPARATOR, HintPart, compact_path, display_session_title, error_message, fit_hint, scrolled_list_top,
    status_color, status_glyph, truncate,
};
use deckhand_core::jsstr::{date, js_len, js_pad_end, path};
use deckhand_core::key::Input;
use deckhand_core::sidebar_model::format_age;
use deckhand_core::tasks_board::{lead_session, worktree_open_items};
use deckhand_core::worktrees_model::{
    WorktreeGroup, WorktreeRow, bulk_targets, delete_refusal, group_title, merged_text, protected_branch,
    selectable_worktree_row, worktree_name, worktree_row_key, worktree_rows, worktree_tags,
};
use deckhand_protocol::data::{
    CleanupInspection, SessionCleanupInspection, SessionRecord, SessionStatus, WorktreeCleanupInspection,
    WorktreeInUse, WorktreeMissing, WorktreeOverview, WorktreeOverviewEntry, WorktreeRemoved,
};
use deckhand_sys::desktop::open_in_editor;

use crate::app::state::App;
use crate::flow::{Flow, FlowCx, FlowEffect};
use crate::keymap::{Binding, HelpRef, KeySpec, Keymap, NamedKey};
use crate::panes::cleanup::cleanup_loss_lines;
use crate::panes::menu::{SelectableRowProps, selectable_row};
use crate::primitives::{
    Border, Column, DialogRows, DialogScroll, DialogView, Row, Seg, TextStyle, Wrap, bordered, dialog_view_fitting,
    dim_area, draw_dialog, fit_row, space_between, truncate_row, with_base,
};
use crate::theme::{THEME, color};

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

impl DeleteConfirm {
    /// The check for one option (TS `deleteBranch ? state.checks.branch : state.checks.worktree`).
    fn check(&self, delete_branch: bool) -> Option<&WorktreeCleanupInspection> {
        if delete_branch {
            self.checks.branch.as_ref()
        } else {
            self.checks.worktree.as_ref()
        }
    }
}

/// `'s'` unless one (TS `${count === 1 ? '' : 's'}`).
fn plural(count: i64) -> &'static str {
    if count == 1 { "" } else { "s" }
}

/// A count as a JS number (counts here are small; saturates instead of wrapping).
fn count(len: usize) -> i64 {
    i64::try_from(len).unwrap_or(i64::MAX)
}

/// A JS string's truthiness (`undefined` and `''` are falsy).
fn truthy(text: Option<&str>) -> bool {
    text.is_some_and(|text| !text.is_empty())
}

/// The inspection's loss data (TS reads the flattened fields of a WorktreeCleanupInspection directly).
fn loss_of(check: &WorktreeCleanupInspection) -> &CleanupInspection {
    &check.inspection.inspection
}

/// `now - Date.parse(at)` as a JS number (NaN for an unreadable time, which formatAge shows as `now`).
fn age_since(now: i64, at: &str) -> f64 {
    match date::parse(at) {
        Some(time) => now.saturating_sub(time) as f64,
        None => f64::NAN,
    }
}

/// TS: worktreesFlow.tsx checkedAgo (`just now` / `3m ago`).
pub fn checked_ago(ms: i64) -> String {
    checked_ago_js(ms as f64)
}

/// [`checked_ago`] with a JS number (NaN: `just now`).
fn checked_ago_js(ms: f64) -> String {
    let age = format_age(ms);
    if age == "now" {
        "just now".to_owned()
    } else {
        format!("{age} ago")
    }
}

/// An Ink color name from deckhand-core as a ratatui color.
fn ink(name: &str) -> Option<ratatui::style::Color> {
    color(name)
}

/// A `<Text wrap="truncate-end">` row of one segment.
fn line(text: impl Into<String>, style: TextStyle, width: usize) -> Row {
    truncate_row(&[Seg::new(text, style)], width, Wrap::TruncateEnd)
}

/// A `<Text>` (Ink's default `wrap="wrap"`) of one segment, as its rows.
fn wrapped(text: impl Into<String>, style: TextStyle, width: usize) -> Vec<Row> {
    fit_row(&[Seg::new(text, style)], width, Wrap::Wrap)
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
    match row {
        WorktreeRow::Empty { text } => line(format!("  {text}"), TextStyle::fg(THEME.muted), width),
        WorktreeRow::Heading { group, count } => {
            let group_color = match group {
                WorktreeGroup::Ready => THEME.success,
                WorktreeGroup::Leftovers | WorktreeGroup::Missing => THEME.warn,
                _ => THEME.muted,
            };
            let text = format!("{} {} · {count}", if selected { "›" } else { " " }, group_title(*group));
            let text = if selected {
                js_pad_end(&format!("{text}  (x deletes them all)"), width, " ")
            } else {
                text
            };
            let style = TextStyle::fg(if selected { THEME.active } else { group_color })
                .inverse(selected)
                .bold(true);
            line(text, style, width)
        }
        WorktreeRow::Worktree { group, entry } => {
            let merged = merged_text(entry, default_branch).filter(|text| !text.is_empty());
            let (glyph, glyph_color) = if *group == WorktreeGroup::Main {
                ("◆", THEME.muted)
            } else if entry.missing.is_some() {
                ("?", THEME.warn)
            } else if merged.is_some() {
                (
                    "✓",
                    if *group == WorktreeGroup::Ready {
                        THEME.success
                    } else {
                        THEME.warn
                    },
                )
            } else if !entry.running_ids.is_empty() {
                ("●", THEME.success)
            } else {
                ("○", THEME.muted)
            };
            let right = truncate(&worktree_tags(entry, sessions, now).join(" · "), width / 2);
            let right_len = js_len(&right);
            let name = truncate(&worktree_name(entry), width.saturating_sub(right_len + 6).max(1));
            let base = TextStyle::default()
                .color(if selected { Some(THEME.active) } else { None })
                .inverse(selected)
                .bold(selected);
            let segs = vec![
                Seg::plain(format!("{} ", if selected { "›" } else { " " })),
                Seg::new(
                    glyph,
                    TextStyle::default().color(if selected { None } else { Some(glyph_color) }),
                ),
                Seg::plain(js_pad_end(
                    &format!(" {name}"),
                    width.saturating_sub(right_len + 3),
                    " ",
                )),
                Seg::new(
                    right,
                    TextStyle::default().color(if selected { None } else { Some(THEME.muted) }),
                ),
            ];
            truncate_row(&with_base(segs, base), width, Wrap::TruncateEnd)
        }
    }
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
    let muted = TextStyle::fg(THEME.muted);
    if let Some(WorktreeRow::Heading {
        group: WorktreeGroup::Ready,
        ..
    }) = row
    {
        let targets = bulk_targets(rows);
        let what = if targets.len() == 1 {
            "this worktree".to_owned()
        } else {
            format!("these {} worktrees", targets.len())
        };
        let names: Vec<String> = targets.iter().map(|entry| worktree_name(entry)).collect();
        return vec![
            line(
                format!("x deletes {what} and their branches (after one confirmation)"),
                TextStyle::default().bold(true),
                width,
            ),
            line(truncate(&names.join(", "), width.saturating_mul(2)), muted, width),
            line(
                "Merged and clean: nothing is lost. Sessions running in them are stopped first.",
                muted,
                width,
            ),
        ];
    }
    let Some(WorktreeRow::Worktree { group, entry }) = row else {
        return Vec::new();
    };
    let in_worktree: Vec<&SessionRecord> = sessions
        .iter()
        .copied()
        .filter(|session| entry.session_ids.contains(&session.id))
        .collect();
    let merged = merged_text(entry, default_branch).filter(|text| !text.is_empty());
    let state = if *group == WorktreeGroup::Main {
        "the main checkout".to_owned()
    } else if let Some(merged) = merged.clone() {
        merged
    } else if entry.missing.is_some() {
        "its directory is gone".to_owned()
    } else if *group == WorktreeGroup::Idle {
        "not merged, idle".to_owned()
    } else {
        "not merged".to_owned()
    };
    let inspection = entry.inspection.as_ref();
    let loss = if entry.is_main || entry.missing.is_some() {
        None
    } else if let Some(error) = entry.error.as_deref().filter(|error| !error.is_empty()) {
        Some(format!("Could not check it: {error}"))
    } else {
        Some(match inspection {
            None => "Not checked".to_owned(),
            Some(inspection) if inspection.safe => "Clean: deleting it and its branch loses nothing".to_owned(),
            Some(inspection) => format!("Deleting it would lose: {}", inspection.reasons.join("; ")),
        })
    };
    let mut notes: Vec<String> = Vec::new();
    match entry.in_use {
        Some(WorktreeInUse::This) => notes.push("this Deckhand runs here".to_owned()),
        Some(WorktreeInUse::Other) => notes.push("another Deckhand is open here".to_owned()),
        None => {}
    }
    if entry.locked == Some(true) {
        notes.push("locked".to_owned());
    }
    if let Some(at) = entry.last_commit_at.as_deref().filter(|at| !at.is_empty()) {
        notes.push(format!("last commit {} ago", format_age(age_since(now, at))));
    }
    if let Some(ahead) = entry.ahead_of_default
        && !entry.is_main
    {
        notes.push(format!(
            "{ahead} commit{} not in {}",
            plural(ahead),
            default_branch.unwrap_or("the default branch")
        ));
    }
    let title = vec![
        Seg::new(worktree_name(entry), TextStyle::default().bold(true)),
        Seg::new(
            format!(" · {state}"),
            TextStyle::fg(if merged.is_some() { THEME.success } else { THEME.muted }),
        ),
    ];
    let place = format!(
        "{}{}",
        compact_path(&entry.path, width),
        if notes.is_empty() {
            String::new()
        } else {
            format!(" · {}", notes.join(" · "))
        }
    );
    let sessions_row = if in_worktree.is_empty() {
        let text = if truthy(entry.record_id.as_deref()) {
            "No sessions left in it"
        } else if entry.is_main {
            "No sessions"
        } else {
            "Not created or attached by Deckhand"
        };
        line(text, muted, width)
    } else {
        let mut segs = Vec::new();
        for (index, session) in in_worktree.iter().take(6).enumerate() {
            if index > 0 {
                segs.push(Seg::plain(", "));
            }
            segs.push(Seg::new(
                status_glyph(session, spinner_frame),
                TextStyle::default().color(ink(status_color(session))),
            ));
            segs.push(Seg::plain(format!(" {}", display_session_title(session, sessions))));
        }
        if in_worktree.len() > 6 {
            segs.push(Seg::new(format!(" +{}", in_worktree.len() - 6), muted));
        }
        truncate_row(&segs, width, Wrap::TruncateEnd)
    };
    let mut lines = vec![
        truncate_row(&title, width, Wrap::TruncateEnd),
        line(place, muted, width),
        sessions_row,
    ];
    if let Some(loss) = loss {
        let safe = inspection.is_some_and(|inspection| inspection.safe);
        lines.push(line(
            loss,
            TextStyle::fg(if safe { THEME.success } else { THEME.warn }),
            width,
        ));
    }
    lines.truncate(DETAIL_ROWS);
    lines
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
    /// Not in the TS: the checks are not all in yet, so no choices are shown (see [`WorktreesFlow::confirm_loading`]).
    pub loading: bool,
}

/// TS: worktreesFlow.tsx ConfirmPane, as a dialog `width` wide: the question (and the path) on top; what goes and
/// what it would lose, in full, in the body; the choices at the bottom (the keys are in the footer).
fn confirm_pane_rows(props: &ConfirmPaneProps<'_>, width: usize) -> DialogRows {
    let inner = width.saturating_sub(4).max(1);
    let state = props.state;
    let options = props.options;
    let index = state.index.min(options.len().saturating_sub(1));
    let Some(entry) = state.entries.first() else {
        return DialogRows::default();
    };
    let running: Vec<&SessionRecord> = props
        .sessions
        .iter()
        .copied()
        .filter(|session| {
            session.status != SessionStatus::Exited
                && state
                    .entries
                    .iter()
                    .any(|entry| entry.session_ids.contains(&session.id))
        })
        .collect();
    let muted = TextStyle::fg(THEME.muted);
    let warn = TextStyle::fg(THEME.warn);
    let title_style = TextStyle::fg(THEME.error).bold(true);
    let mut rows = DialogRows::default();
    if let Some(override_) = state.override_.as_ref() {
        let check = state.check(override_.delete_branch);
        rows.header.extend(wrapped(
            format!("Delete {} anyway?", worktree_name(entry)),
            title_style,
            inner,
        ));
        rows.body.extend(wrapped(
            "This permanently erases what is listed below. Main/current/shared worktree protections cannot be overridden.",
            TextStyle::default(),
            inner,
        ));
        rows.body.push(Vec::new());
        match check {
            Some(check) => rows.body.extend(cleanup_loss_lines(loss_of(check), inner, usize::MAX)),
            None => rows.body.extend(wrapped("Safety could not be verified", warn, inner)),
        }
        rows.footer.push(Vec::new());
        rows.footer.extend(wrapped(
            format!("Type DELETE then enter: {}", override_.draft),
            warn,
            inner,
        ));
        return rows;
    }
    let check = if state.bulk {
        None
    } else {
        state.check(
            options
                .get(index)
                .is_some_and(|option| option.kind == ConfirmKind::DeleteBranch),
        )
    };
    let summary = if state.bulk || entry.missing.is_some() {
        None
    } else {
        Some(match check {
            None => "Checking what it would lose…",
            Some(check) if !check.inspection.structural_blockers.is_empty() => "Deletion is blocked:",
            Some(check) if loss_of(check).safe => "Local cleanup checks passed: nothing is lost",
            Some(_) => "Deleting it would lose (typing DELETE overrides):",
        })
    };
    let title = if state.bulk {
        let count = state.entries.len();
        format!("Delete {count} merged worktree{}?", plural(self::count(count)))
    } else if entry.missing.is_some() {
        format!("Clean up {}?", worktree_name(entry))
    } else {
        format!("Delete {}?", worktree_name(entry))
    };
    rows.header.extend(wrapped(title, title_style, inner));
    if state.bulk {
        for item in &state.entries {
            let name = worktree_name(item);
            let room = count(inner)
                .saturating_sub(count(js_len(&name)))
                .saturating_sub(5)
                .max(8);
            let room = usize::try_from(room).unwrap_or(8);
            rows.body.push(line(
                format!("  {name} · {}", compact_path(&item.path, room)),
                muted,
                inner,
            ));
        }
    } else {
        rows.header.push(line(compact_path(&entry.path, inner), muted, inner));
    }
    if let Some(summary) = summary {
        let safe = check.is_some_and(|check| loss_of(check).safe);
        rows.body.push(Vec::new());
        rows.body.extend(wrapped(
            summary,
            TextStyle::fg(if safe { THEME.success } else { THEME.warn }),
            inner,
        ));
    }
    // What it would lose, in full: the reasons, then every file.
    if let Some(check) = check {
        let loss = loss_of(check);
        if !loss.safe && check.inspection.structural_blockers.is_empty() {
            rows.body.extend(cleanup_loss_lines(loss, inner, usize::MAX));
        }
        for blocker in &check.inspection.structural_blockers {
            rows.body.extend(wrapped(format!("  {blocker}"), warn, inner));
        }
    }
    if !running.is_empty() {
        let who = if running.len() == 1 {
            "the session".to_owned()
        } else {
            format!("{} sessions", running.len())
        };
        let titles: Vec<String> = running
            .iter()
            .map(|session| display_session_title(session, props.sessions))
            .collect();
        rows.body.extend(wrapped(
            format!("Stops {who} running there first: {}", titles.join(", ")),
            warn,
            inner,
        ));
    }
    if !props.open_items.is_empty() {
        let keep = state.keep_open_items;
        let items = props.open_items.len();
        let text = format!(
            "{} {items} open note item{} {}: {}",
            if keep { "☐" } else { "☑" },
            plural(count(items)),
            if keep {
                "stay in the (read-only) note"
            } else {
                "→ backlog first"
            },
            props.open_items.join(", ")
        );
        rows.body.extend(wrapped(
            text,
            TextStyle::fg(if keep { THEME.muted } else { THEME.active }),
            inner,
        ));
    }
    if entry.missing.is_none() {
        rows.body.extend(wrapped(
            "Its sessions cannot be resumed afterwards; they are archived (notes kept, f A shows them).",
            muted,
            inner,
        ));
    }
    if props.loading {
        return rows;
    }
    rows.footer.push(Vec::new());
    for (row, option) in options.iter().enumerate() {
        let selected_color = if option.kind == ConfirmKind::Cancel {
            THEME.muted
        } else {
            THEME.error
        };
        rows.footer.extend(selectable_row(
            &SelectableRowProps {
                selected: row == index,
                text: &option.label,
                selected_color: Some(selected_color),
                wrap: true,
                ..SelectableRowProps::default()
            },
            inner,
        ));
    }
    if let Some(working) = props.working {
        rows.footer.push(Vec::new());
        rows.footer.extend(wrapped(working, warn, inner));
    }
    rows
}

/// TS: worktreesFlow.tsx ConfirmPane.
pub fn render_confirm_pane(
    props: &ConfirmPaneProps<'_>,
    buf: &mut Buffer,
    area: Rect,
    view: DialogView,
) -> DialogScroll {
    let build = |width| confirm_pane_rows(props, usize::from(width));
    draw_dialog(buf, area, &Border::round(THEME.border_danger), &build, view)
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
    /// Not in the TS: how far the confirmation's body is scrolled (J/K, PgUp/PgDn), and how it scrolled when last drawn.
    pub confirm_scroll: (usize, DialogScroll),
}

/// The worktree manager's keys (keysDocumented: each must be in the guide's Worktrees topic).
pub static KEYMAP: Keymap = &[
    Binding {
        keys: &[KeySpec::Named(NamedKey::Escape)],
        action: "back to the session list (in a confirmation: cancel, or leave the DELETE prompt)",
        help: HelpRef::Hidden("esc goes back everywhere; the footer hint says so"),
    },
    Binding {
        keys: &[KeySpec::Char("k"), KeySpec::Named(NamedKey::Up)],
        action: "move up (in a confirmation: the previous option)",
        help: HelpRef::Topic("Worktrees"),
    },
    Binding {
        keys: &[KeySpec::Char("j"), KeySpec::Named(NamedKey::Down)],
        action: "move down (in a confirmation: the next option)",
        help: HelpRef::Topic("Worktrees"),
    },
    Binding {
        keys: &[KeySpec::Named(NamedKey::PageUp), KeySpec::Named(NamedKey::PageDown)],
        action: "move ten rows",
        help: HelpRef::Hidden("page keys move as in every list"),
    },
    Binding {
        keys: &[KeySpec::Char("g"), KeySpec::Named(NamedKey::Home)],
        action: "first worktree",
        help: HelpRef::Topic("Worktrees"),
    },
    Binding {
        keys: &[KeySpec::Char("G"), KeySpec::Named(NamedKey::End)],
        action: "last worktree",
        help: HelpRef::Topic("Worktrees"),
    },
    Binding {
        keys: &[KeySpec::Char("R")],
        action: "check them all again",
        help: HelpRef::Topic("Worktrees"),
    },
    Binding {
        keys: &[KeySpec::Char("x")],
        action: "delete it (on the ready heading: all of them), asking first",
        help: HelpRef::Topic("Worktrees"),
    },
    Binding {
        keys: &[KeySpec::Named(NamedKey::Return), KeySpec::Char("o")],
        action: "select its session (in a confirmation: choose)",
        help: HelpRef::Topic("Worktrees"),
    },
    Binding {
        keys: &[KeySpec::Char("E")],
        action: "open it in your editor",
        help: HelpRef::Topic("Worktrees"),
    },
    Binding {
        keys: &[KeySpec::Char("M")],
        action: "mark it merged or clear the marker",
        help: HelpRef::Topic("Worktrees"),
    },
    Binding {
        keys: &[KeySpec::Char(" ")],
        action: "in a confirmation: keep or send the notes' open items",
        help: HelpRef::Hidden("shown in the confirmation's footer hint"),
    },
    Binding {
        keys: &[KeySpec::Named(NamedKey::Backspace), KeySpec::Named(NamedKey::Delete)],
        action: "in the DELETE prompt: erase a letter",
        help: HelpRef::Hidden("typing DELETE is explained by the prompt itself"),
    },
    Binding {
        keys: &[KeySpec::Char("J"), KeySpec::Char("K")],
        action: "scroll a deletion confirmation that does not fit (PgUp/PgDn too)",
        help: HelpRef::Topic("Worktrees"),
    },
];

/// The rows as shown now and where the selection is (TS `rows`, `selectable`, `current`).
struct View {
    rows: Vec<WorktreeRow>,
    selectable: Vec<usize>,
    current: Option<usize>,
}

impl View {
    fn current_row(&self) -> Option<&WorktreeRow> {
        self.current.and_then(|index| self.rows.get(index))
    }

    /// TS `currentEntry`.
    fn current_entry(&self) -> Option<&WorktreeOverviewEntry> {
        match self.current_row() {
            Some(WorktreeRow::Worktree { entry, .. }) => Some(entry),
            _ => None,
        }
    }

    /// Whether the selection is the ready group's heading (x there deletes them all).
    fn on_ready(&self) -> bool {
        matches!(
            self.current_row(),
            Some(WorktreeRow::Heading {
                group: WorktreeGroup::Ready,
                ..
            })
        )
    }
}

/// One worktree of a bulk deletion, as decided when it started.
struct BulkJob {
    name: String,
    path: String,
    branch: String,
    delete_branch: bool,
    send_open_items: bool,
}

/// TS deleteAll's loop state (the `for` over the entries with its counters).
struct BulkRun {
    client: Rc<LiveClient>,
    cwd: String,
    jobs: Vec<BulkJob>,
    index: usize,
    deleted: i64,
    branches: i64,
    stopped: i64,
    sent: i64,
    failed: Vec<String>,
}

/// TS `sessionsOf(entry)`.
fn sessions_of<'a>(entry: &WorktreeOverviewEntry, sessions: &[&'a SessionRecord]) -> Vec<&'a SessionRecord> {
    sessions
        .iter()
        .copied()
        .filter(|session| entry.session_ids.contains(&session.id))
        .collect()
}

/// TS `runningOf(entry)`.
fn running_of<'a>(entry: &WorktreeOverviewEntry, sessions: &[&'a SessionRecord]) -> Vec<&'a SessionRecord> {
    sessions_of(entry, sessions)
        .into_iter()
        .filter(|session| session.status != SessionStatus::Exited)
        .collect()
}

/// TS `openItemsOf(entry)`.
fn open_items_of(entry: &WorktreeOverviewEntry, sessions: &[&SessionRecord]) -> Vec<String> {
    worktree_open_items(entry.record_id.as_deref(), sessions)
}

/// TS `sendsItems(entry, keep)`.
fn sends_items(entry: &WorktreeOverviewEntry, keep: bool, sessions: &[&SessionRecord]) -> bool {
    !keep && !open_items_of(entry, sessions).is_empty()
}

/// TS `sentText(count)`.
fn sent_text(count: i64) -> String {
    if count == 0 {
        String::new()
    } else {
        format!("{count} open note item{} to the backlog", plural(count))
    }
}

/// The fallback check when the daemon could not inspect it (TS `satisfies WorktreeCleanupInspection`).
fn failed_check(message: String) -> WorktreeCleanupInspection {
    WorktreeCleanupInspection {
        inspection: SessionCleanupInspection {
            inspection: CleanupInspection {
                safe: false,
                reasons: vec![message],
                dirty_files: 0,
                untracked_files: 0,
                ignored_files: 0,
                files: None,
                unpublished_commits: None,
                unmerged_commits: None,
            },
            structural_blockers: Vec::new(),
        },
        running: Vec::new(),
    }
}

/// `text` with its first letter upper-cased (TS `${text[0]!.toUpperCase()}${text.slice(1)}`).
fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

impl WorktreesFlow {
    /// TS: WorktreesFlow.open (W): loads the list, the selection on `select_path` when listed.
    pub fn open(&mut self, select_path: Option<&str>, props: &WorktreesFlowProps<'_>, cx: &mut FlowCx<App, Self>) {
        self.confirm = None;
        self.selected = match select_path.filter(|path| !path.is_empty()) {
            Some(path) => (Some(format!("wt:{path}")), 0),
            None => (None, 0),
        };
        self.load(select_path, props, cx);
    }

    /// TS: useWorktreesFlow load.
    pub fn load(&mut self, select_path: Option<&str>, props: &WorktreesFlowProps<'_>, cx: &mut FlowCx<App, Self>) {
        self.load_with(
            props.client.cloned(),
            props.cwd.to_owned(),
            select_path.map(str::to_owned),
            cx,
        );
    }

    /// [`WorktreesFlow::load`] with what it needs owned (continuations call it after their props are gone).
    fn load_with(
        &mut self,
        client: Option<Rc<LiveClient>>,
        cwd: String,
        select_path: Option<String>,
        cx: &mut FlowCx<App, Self>,
    ) {
        let Some(client) = client else {
            cx.set_error("still connecting to daemon".to_owned());
            return;
        };
        self.load_id += 1;
        let id = self.load_id;
        self.loading = true;
        let reply = client.worktree_overview(&cwd);
        drop(cx.spawn(reply, move |flow: &mut WorktreesFlow, result, cx| {
            if flow.load_id != id {
                return;
            }
            match result {
                Ok(next) => {
                    flow.overview = Some(next);
                    if let Some(path) = select_path.filter(|path| !path.is_empty()) {
                        flow.selected = (Some(format!("wt:{path}")), 0);
                    }
                }
                Err(error) => cx.set_error(error_message(&error)),
            }
            flow.loading = false;
        }));
    }

    /// The rows as shown now, running sessions from the live list (TS `rows`).
    pub fn rows(&self, props: &WorktreesFlowProps<'_>, now: i64) -> Vec<WorktreeRow> {
        // Which sessions run comes from the live session list, not the (older) overview.
        let live = self.overview.as_ref().map(|overview| {
            let mut live = overview.clone();
            for entry in &mut live.entries {
                entry.running_ids = entry
                    .session_ids
                    .iter()
                    .filter(|id| {
                        props
                            .sessions
                            .iter()
                            .find(|session| &session.id == *id)
                            .is_some_and(|session| session.status != SessionStatus::Exited)
                    })
                    .cloned()
                    .collect();
            }
            live
        });
        worktree_rows(live.as_ref(), props.sessions, now)
    }

    /// TS: the derived `rows`, `selectable` and `current` of a render.
    fn view(&self, props: &WorktreesFlowProps<'_>, now: i64) -> View {
        let rows = self.rows(props, now);
        let selectable: Vec<usize> = rows
            .iter()
            .enumerate()
            .filter(|(_, row)| selectable_worktree_row(row))
            .map(|(index, _)| index)
            .collect();
        let keyed = self
            .selected
            .0
            .as_deref()
            .filter(|key| !key.is_empty())
            .and_then(|key| {
                rows.iter()
                    .position(|row| worktree_row_key(row).as_deref() == Some(key))
            });
        let current = keyed.or_else(|| {
            selectable
                .get(self.selected.1.min(selectable.len().saturating_sub(1)))
                .copied()
        });
        View {
            rows,
            selectable,
            current,
        }
    }

    /// TS `select(index)`.
    fn select(&mut self, view: &View, index: usize) {
        let Some(row) = view.rows.get(index) else {
            return;
        };
        let position = view.selectable.iter().position(|&at| at == index).unwrap_or(0);
        self.selected = (worktree_row_key(row), position);
    }

    /// TS `move(direction)`.
    fn move_by(&mut self, view: &View, direction: i64) {
        if view.selectable.is_empty() {
            return;
        }
        let at = view
            .current
            .and_then(|current| view.selectable.iter().position(|&index| index == current))
            .unwrap_or(0);
        let last = count(view.selectable.len()) - 1;
        let next = (count(at) + direction).clamp(0, last);
        if let Some(&index) = usize::try_from(next).ok().and_then(|next| view.selectable.get(next)) {
            self.select(view, index);
        }
    }

    /// Not in the TS: the deletion confirmation of one worktree is still checking what deleting it (and, when it
    /// could go too, its branch) would lose. No choices are shown until both answers are in: drawn early, they moved
    /// when the file list arrived. A bulk deletion and a missing worktree check nothing.
    pub fn confirm_loading(&self, state: &DeleteConfirm) -> bool {
        match state.entries.first() {
            Some(entry) if !state.bulk && entry.missing.is_none() => {
                state.checks.worktree.is_none() || (self.can_delete_branch(entry) && state.checks.branch.is_none())
            }
            _ => false,
        }
    }

    /// TS `canDeleteBranch(entry)`.
    fn can_delete_branch(&self, entry: &WorktreeOverviewEntry) -> bool {
        !entry.branch.is_empty() && !protected_branch(&entry.branch, self.default_branch())
    }

    /// TS `overview?.defaultBranch`.
    fn default_branch(&self) -> Option<&str> {
        self.overview
            .as_ref()
            .and_then(|overview| overview.default_branch.as_deref())
    }

    /// TS: useWorktreesFlow openConfirm: the deletion confirmation, with fresh checks of one worktree.
    pub fn open_confirm(
        &mut self,
        entries: Vec<WorktreeOverviewEntry>,
        bulk: bool,
        props: &WorktreesFlowProps<'_>,
        cx: &mut FlowCx<App, Self>,
    ) {
        self.check_id += 1;
        let id = self.check_id;
        let entry = entries.first().cloned();
        self.confirm_scroll = (0, DialogScroll::default());
        self.confirm = Some(DeleteConfirm {
            entries,
            bulk,
            index: 0,
            checks: ConfirmChecks::default(),
            override_: None,
            keep_open_items: false,
        });
        let Some(entry) = entry else {
            return;
        };
        if bulk || entry.missing.is_some() {
            return;
        }
        let Some(client) = props.client else {
            return;
        };
        let check = |delete_branch: bool| {
            let reply = client.inspect_worktree(props.cwd, &entry.path, delete_branch);
            let checked = async move { reply.await.unwrap_or_else(|error| failed_check(error_message(&error))) };
            drop(cx.spawn(checked, move |flow: &mut WorktreesFlow, result, _cx| {
                if flow.check_id != id {
                    return;
                }
                if let Some(state) = flow.confirm.as_mut() {
                    if delete_branch {
                        state.checks.branch = Some(result);
                    } else {
                        state.checks.worktree = Some(result);
                    }
                }
            }));
        };
        check(false);
        if self.can_delete_branch(&entry) {
            check(true);
        }
    }

    /// TS: useWorktreesFlow options: the confirmation's choices.
    pub fn options(&self, state: &DeleteConfirm, props: &WorktreesFlowProps<'_>) -> Vec<ConfirmOption> {
        let cancel = ConfirmOption {
            kind: ConfirmKind::Cancel,
            label: "Cancel".to_owned(),
        };
        let running: usize = state
            .entries
            .iter()
            .map(|entry| running_of(entry, props.sessions).len())
            .sum();
        let stop = if running > 0 {
            format!("Stop {running} session{}, ", plural(count(running)))
        } else {
            String::new()
        };
        let verb = |text: &str| {
            if stop.is_empty() {
                capitalize(text)
            } else {
                format!("{stop}{text}")
            }
        };
        let option = |kind: ConfirmKind, label: String| ConfirmOption { kind, label };
        if state.bulk {
            let total = state.entries.len();
            let noun = format!("{total} worktree{}", plural(count(total)));
            return vec![
                option(
                    ConfirmKind::DeleteBranch,
                    verb(&format!("delete {noun} and their branches")),
                ),
                option(ConfirmKind::Delete, verb(&format!("delete {noun}, keep the branches"))),
                cancel,
            ];
        }
        let Some(entry) = state.entries.first() else {
            return vec![cancel];
        };
        match entry.missing {
            Some(WorktreeMissing::Prunable) => {
                return vec![
                    option(
                        ConfirmKind::Prune,
                        "Prune it: Git forgets the worktree (its branch stays)".to_owned(),
                    ),
                    cancel,
                ];
            }
            Some(WorktreeMissing::Unlisted) => {
                return vec![
                    option(
                        ConfirmKind::Forget,
                        "Forget it: Deckhand's record only, nothing on disk".to_owned(),
                    ),
                    cancel,
                ];
            }
            None => {}
        }
        let keep = option(
            ConfirmKind::Delete,
            verb(if entry.branch.is_empty() {
                "delete the worktree"
            } else {
                "delete the worktree, keep its branch"
            }),
        );
        if !self.can_delete_branch(entry) {
            return vec![keep, cancel];
        }
        let with_branch = option(
            ConfirmKind::DeleteBranch,
            verb(&format!("delete the worktree and branch {}", entry.branch)),
        );
        // Merged work's branch has nothing left to keep: offered first.
        if merged_text(entry, self.default_branch()).is_some_and(|text| !text.is_empty()) {
            vec![with_branch, keep, cancel]
        } else {
            vec![keep, with_branch, cancel]
        }
    }

    /// TS `finish(message)`.
    fn finish(&mut self, message: String, client: Rc<LiveClient>, cwd: String, cx: &mut FlowCx<App, Self>) {
        self.working = None;
        self.confirm = None;
        cx.set_status(message);
        self.load_with(Some(client), cwd, None, cx);
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
        let Some(client) = props.client.cloned() else {
            return;
        };
        if self.working.is_some() {
            return;
        }
        let name = worktree_name(entry);
        self.working = Some(format!("Deleting {name}…"));
        cx.set_error(None);
        let options = DeleteWorktreeOptions {
            branch: Some(entry.branch.clone()),
            delete_branch: Some(delete_branch),
            stop_sessions: Some(true),
            allow_data_loss: Some(allow_data_loss),
            send_open_items: Some(sends_items(entry, keep_open_items, props.sessions)),
        };
        let reply = client.delete_worktree(props.cwd, &entry.path, options);
        let cwd = props.cwd.to_owned();
        drop(
            cx.spawn(reply, move |flow: &mut WorktreesFlow, result, cx| match result {
                Ok(result) => {
                    let done = match result.removed {
                        WorktreeRemoved::Deleted => format!(
                            "Deleted {name}{}",
                            if truthy(result.branch_deleted.as_deref()) {
                                " and its branch"
                            } else {
                                ""
                            }
                        ),
                        WorktreeRemoved::Pruned => format!("Pruned {name}"),
                        WorktreeRemoved::Forgotten => format!("Forgot {name}"),
                    };
                    let stopped = if result.stopped != 0 {
                        format!("stopped {} session{}", result.stopped, plural(result.stopped))
                    } else {
                        String::new()
                    };
                    let archived = if result.archived != 0 {
                        format!("archived {} (f A shows them)", result.archived)
                    } else {
                        String::new()
                    };
                    let parts: Vec<String> = [done, stopped, sent_text(result.sent_items.unwrap_or(0)), archived]
                        .into_iter()
                        .filter(|part| !part.is_empty())
                        .collect();
                    flow.finish(parts.join(" · "), client, cwd, cx);
                }
                Err(error) => {
                    flow.working = None;
                    flow.confirm = None;
                    cx.set_error(error_message(&error));
                    flow.load_with(Some(client), cwd, None, cx);
                }
            }),
        );
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
        let Some(client) = props.client.cloned() else {
            return;
        };
        if self.working.is_some() {
            return;
        }
        cx.set_error(None);
        // What TS reads from the render the loop started in (overview and sessions), decided up front.
        let jobs = entries
            .iter()
            .map(|entry| BulkJob {
                name: worktree_name(entry),
                path: entry.path.clone(),
                branch: entry.branch.clone(),
                delete_branch: with_branches && self.can_delete_branch(entry),
                send_open_items: sends_items(entry, keep_open_items, props.sessions),
            })
            .collect();
        let run = BulkRun {
            client,
            cwd: props.cwd.to_owned(),
            jobs,
            index: 0,
            deleted: 0,
            branches: 0,
            stopped: 0,
            sent: 0,
            failed: Vec::new(),
        };
        self.bulk_step(run, cx);
    }

    /// One turn of deleteAll's loop: the next deletion, or the summary once all are done.
    fn bulk_step(&mut self, mut run: BulkRun, cx: &mut FlowCx<App, Self>) {
        let total = run.jobs.len();
        let Some(job) = run.jobs.get(run.index) else {
            if !run.failed.is_empty() {
                cx.set_error(format!("Not deleted: {}", run.failed.join("; ")));
            }
            let deleted = format!(
                "Deleted {} worktree{}{}",
                run.deleted,
                plural(run.deleted),
                if run.branches != 0 {
                    format!(
                        " and {} branch{}",
                        run.branches,
                        if run.branches == 1 { "" } else { "es" }
                    )
                } else {
                    String::new()
                }
            );
            let stopped = if run.stopped != 0 {
                format!("stopped {} session{}", run.stopped, plural(run.stopped))
            } else {
                String::new()
            };
            let parts: Vec<String> = [deleted, stopped, sent_text(run.sent)]
                .into_iter()
                .filter(|part| !part.is_empty())
                .collect();
            self.finish(parts.join(" · "), run.client, run.cwd, cx);
            return;
        };
        self.working = Some(format!("Deleting {}… {}/{total}", job.name, run.index + 1));
        let options = DeleteWorktreeOptions {
            branch: Some(job.branch.clone()),
            delete_branch: Some(job.delete_branch),
            stop_sessions: Some(true),
            allow_data_loss: None,
            send_open_items: Some(job.send_open_items),
        };
        let reply = run.client.delete_worktree(&run.cwd, &job.path, options);
        let name = job.name.clone();
        drop(cx.spawn(reply, move |flow: &mut WorktreesFlow, result, cx| {
            match result {
                Ok(result) => {
                    run.deleted += 1;
                    run.stopped += result.stopped;
                    run.sent += result.sent_items.unwrap_or(0);
                    if truthy(result.branch_deleted.as_deref()) {
                        run.branches += 1;
                    }
                }
                Err(error) => run.failed.push(format!("{name}: {}", error_message(&error))),
            }
            run.index += 1;
            flow.bulk_step(run, cx);
        }));
    }

    /// TS `confirmInput(input, key, state)`.
    fn confirm_input(&mut self, props: &WorktreesFlowProps<'_>, input: &Input, cx: &mut FlowCx<App, Self>) {
        if self.working.is_some() {
            return;
        }
        let Some(mut state) = self.confirm.clone() else {
            return;
        };
        let key = &input.key;
        let text = input.input.as_str();
        // J/K (PgUp/PgDn) scroll what does not fit, except while DELETE is typed.
        if let Some(up) = crate::keymap::dialog_scroll(input).filter(|_| state.override_.is_none()) {
            let (scroll, last) = self.confirm_scroll;
            self.confirm_scroll.0 = last.paged(scroll, up);
            return;
        }
        // Until the checks are in, the choices are not shown: only Esc acts.
        if self.confirm_loading(&state) {
            if key.escape {
                self.confirm = None;
            } else if key.return_ {
                cx.set_status("Still checking what it would lose…".to_owned());
            }
            return;
        }
        let list = self.options(&state, props);
        let len = list.len();
        let index = state.index.min(len.saturating_sub(1));
        if let Some(mut override_) = state.override_.take() {
            if key.escape {
                self.confirm = Some(state);
                return;
            }
            if key.backspace || key.delete {
                override_.draft.pop();
                state.override_ = Some(override_);
                self.confirm = Some(state);
                return;
            }
            if key.return_ {
                if override_.draft == "DELETE"
                    && let Some(entry) = state.entries.first()
                {
                    self.delete_one(entry, override_.delete_branch, true, state.keep_open_items, props, cx);
                }
                return;
            }
            if !key.ctrl && !key.meta && !text.is_empty() && text.chars().all(|c| c.is_ascii_alphabetic()) {
                override_.draft.push_str(text);
                override_.draft.truncate(10);
                state.override_ = Some(override_);
                self.confirm = Some(state);
            }
            return;
        }
        if key.escape {
            self.confirm = None;
            return;
        }
        if len == 0 {
            return;
        }
        if key.up_arrow || text == "k" {
            state.index = (index + len - 1) % len;
            self.confirm = Some(state);
            return;
        }
        if key.down_arrow || text == "j" {
            state.index = (index + 1) % len;
            self.confirm = Some(state);
            return;
        }
        if text == " " {
            if state
                .entries
                .iter()
                .any(|entry| !open_items_of(entry, props.sessions).is_empty())
            {
                state.keep_open_items = !state.keep_open_items;
                self.confirm = Some(state);
            }
            return;
        }
        if !key.return_ {
            return;
        }
        let Some(option) = list.get(index) else {
            return;
        };
        if option.kind == ConfirmKind::Cancel {
            self.confirm = None;
            return;
        }
        if state.bulk {
            let with_branches = option.kind == ConfirmKind::DeleteBranch;
            self.delete_all(state.entries, with_branches, state.keep_open_items, props, cx);
            return;
        }
        let Some(entry) = state.entries.first() else {
            return;
        };
        if matches!(option.kind, ConfirmKind::Prune | ConfirmKind::Forget) {
            self.delete_one(entry, false, false, state.keep_open_items, props, cx);
            return;
        }
        let delete_branch = option.kind == ConfirmKind::DeleteBranch;
        let Some(check) = state.check(delete_branch) else {
            cx.set_status("Still checking what it would lose…".to_owned());
            return;
        };
        if !check.inspection.structural_blockers.is_empty() {
            cx.set_error(check.inspection.structural_blockers.join("; "));
            return;
        }
        if loss_of(check).safe {
            self.delete_one(entry, delete_branch, false, state.keep_open_items, props, cx);
        } else {
            state.override_ = Some(DeleteOverride {
                delete_branch,
                draft: String::new(),
            });
            self.confirm = Some(state);
        }
    }

    /// The manager itself (TS render without a confirmation): the rows inside its border.
    fn list_rows(&mut self, props: &WorktreesFlowProps<'_>, width: usize, height: usize, now: i64) -> Vec<Row> {
        let view = self.view(props, now);
        let rows = &view.rows;
        let inner = width.saturating_sub(4).max(10);
        let default_branch = self.default_branch().map(str::to_owned);
        let default_branch = default_branch.as_deref();
        let muted = TextStyle::fg(THEME.muted);
        let linked = self.overview.as_ref().map_or(0, |overview| {
            overview.entries.iter().filter(|entry| !entry.is_main).count()
        });
        let ready = rows
            .iter()
            .filter(|row| {
                matches!(
                    row,
                    WorktreeRow::Worktree {
                        group: WorktreeGroup::Ready,
                        ..
                    }
                )
            })
            .count();
        let right = match self.overview {
            Some(_) => format!(
                "{linked} worktree{}{}",
                plural(count(linked)),
                if ready > 0 {
                    format!(" · {ready} ready to delete")
                } else {
                    String::new()
                }
            ),
            None => String::new(),
        };
        let status = match (&self.working, &self.overview) {
            (Some(working), _) => working.clone(),
            (None, _) if self.loading => "Checking worktrees…".to_owned(),
            (None, Some(overview)) => format!(
                "{} · checked {}{}",
                match overview.default_branch.as_deref() {
                    Some(branch) if !branch.is_empty() => format!("Compared with {branch}"),
                    _ => "No default branch found".to_owned(),
                },
                checked_ago_js(age_since(now, &overview.checked_at)),
                match overview.unchecked {
                    Some(unchecked) if unchecked != 0 => format!(" · {unchecked} not checked (too many)"),
                    _ => String::new(),
                }
            ),
            (None, None) => String::new(),
        };
        let lower = detail_lines(
            view.current_row(),
            rows,
            props.sessions,
            default_branch,
            props.spinner_frame,
            now,
            inner,
        );
        let reserved = 2 + 3 + if lower.is_empty() { 0 } else { lower.len() + 1 };
        let list_rows = height.saturating_sub(reserved).max(1);
        self.top = scrolled_list_top(self.top, view.current, list_rows, rows.len(), &|index| {
            rows.get(index).is_some_and(selectable_worktree_row)
        });
        let top = self.top;

        let mut out: Vec<Row> = Vec::new();
        let root = self
            .overview
            .as_ref()
            .and_then(|overview| overview.main_root.as_deref())
            .unwrap_or(props.cwd);
        let room = inner.saturating_sub(js_len(&right)).saturating_sub(16).max(4);
        let left = vec![
            Seg::new("⎇ Worktrees", TextStyle::fg(THEME.active).bold(true)),
            Seg::new(format!(" · {}", truncate(&path::basename(root), room)), muted),
        ];
        out.push(space_between(&left, &[Seg::new(right, muted)], inner));
        let status = if status.is_empty() { " ".to_owned() } else { status };
        let status_color = if self.working.is_some() {
            THEME.warn
        } else {
            THEME.muted
        };
        out.push(line(status, TextStyle::fg(status_color), inner));
        for offset in 0..list_rows {
            let index = top + offset;
            match rows.get(index) {
                Some(row) => out.push(worktree_line(
                    row,
                    Some(index) == view.current,
                    inner,
                    props.sessions,
                    default_branch,
                    now,
                )),
                None => out.push(Vec::new()),
            }
        }
        if rows.len() > top + list_rows {
            out.extend(wrapped(
                format!("  +{} more", rows.len() - top - list_rows),
                muted,
                inner,
            ));
        } else {
            out.push(vec![Seg::plain(" ")]);
        }
        if !lower.is_empty() {
            out.push(vec![Seg::new("─".repeat(inner), TextStyle::fg(THEME.border))]);
        }
        out.extend(lower);
        out
    }
}

impl Flow<App> for WorktreesFlow {
    type Props<'a> = WorktreesFlowProps<'a>;

    fn handle_input(&mut self, props: &WorktreesFlowProps<'_>, input: &Input, cx: &mut FlowCx<App, Self>) {
        if self.confirm.is_some() {
            self.confirm_input(props, input, cx);
            return;
        }
        let key = &input.key;
        let text = input.input.as_str();
        let view = self.view(props, date::now_ms());
        if key.escape {
            cx.exit();
            return;
        }
        if key.up_arrow || text == "k" {
            self.move_by(&view, -1);
            return;
        }
        if key.down_arrow || text == "j" {
            self.move_by(&view, 1);
            return;
        }
        if key.page_up {
            self.move_by(&view, -10);
            return;
        }
        if key.page_down {
            self.move_by(&view, 10);
            return;
        }
        if (text == "g" || key.home)
            && let Some(&first) = view.selectable.first()
        {
            self.select(&view, first);
            return;
        }
        if (text == "G" || key.end)
            && let Some(&last) = view.selectable.last()
        {
            self.select(&view, last);
            return;
        }
        if text == "R" {
            self.load(None, props, cx);
            cx.set_status("Checking the worktrees again".to_owned());
            return;
        }
        if self.working.is_some() {
            return;
        }
        if text == "x" {
            if view.on_ready() {
                let targets: Vec<WorktreeOverviewEntry> = bulk_targets(&view.rows).into_iter().cloned().collect();
                if targets.is_empty() {
                    cx.set_status("None of them can be deleted from here".to_owned());
                } else {
                    self.open_confirm(targets, true, props, cx);
                }
                return;
            }
            let Some(entry) = view.current_entry() else {
                cx.set_status("Select a worktree first (j/k)".to_owned());
                return;
            };
            if let Some(refusal) = delete_refusal(entry) {
                cx.set_status(refusal.to_owned());
                return;
            }
            self.open_confirm(vec![entry.clone()], false, props, cx);
            return;
        }
        let Some(entry) = view.current_entry() else {
            if key.return_ || text == "o" || text == "E" || text == "M" {
                cx.set_status("Select a worktree first (j/k)".to_owned());
            }
            return;
        };
        if key.return_ || text == "o" {
            let mine = sessions_of(entry, props.sessions);
            match lead_session(&mine) {
                Some(lead) => cx.emit(FlowEffect::GoTo(lead.id.clone())),
                None => cx.set_status(
                    if entry.is_main {
                        "No session in the main checkout"
                    } else {
                        "No session in it: x deletes it, E opens it in your editor"
                    }
                    .to_owned(),
                ),
            }
            return;
        }
        if text == "E" {
            if entry.missing.is_some() {
                cx.set_status("Its directory is gone".to_owned());
                return;
            }
            let errors: RefCell<Vec<String>> = RefCell::new(Vec::new());
            let label = open_in_editor(&entry.path, &|error| errors.borrow_mut().push(error), None);
            for error in errors.into_inner() {
                cx.set_error(error);
            }
            if let Some(label) = label.filter(|label| !label.is_empty()) {
                cx.set_status(format!("Opened {} in {label}", worktree_name(entry)));
            }
            return;
        }
        if text == "M" {
            // The merged marker belongs to the worktree's record; any of its sessions sets or clears it.
            let owner = entry
                .record_id
                .as_deref()
                .filter(|id| !id.is_empty())
                .and_then(|record_id| {
                    sessions_of(entry, props.sessions).into_iter().find(|session| {
                        session.worktree.as_ref().and_then(|worktree| worktree.id.as_deref()) == Some(record_id)
                    })
                });
            let (Some(owner), Some(client)) = (owner, props.client) else {
                cx.set_status(
                    if entry.is_main {
                        "The main checkout has nothing to merge"
                    } else {
                        "Only worktrees Deckhand tracks have a merged marker"
                    }
                    .to_owned(),
                );
                return;
            };
            let was_merged = truthy(entry.markers.as_ref().and_then(|markers| markers.merged_at.as_deref()));
            let name = worktree_name(entry);
            let reply = client.mark_session_merged(&owner.id, props.cwd);
            let client = Rc::clone(client);
            let cwd = props.cwd.to_owned();
            drop(
                cx.spawn(reply, move |flow: &mut WorktreesFlow, result, cx| match result {
                    Ok(_) => {
                        cx.set_status(if was_merged {
                            format!("Cleared the merged marker of {name}")
                        } else {
                            format!("Marked {name} merged")
                        });
                        flow.load_with(Some(client), cwd, None, cx);
                    }
                    Err(error) => cx.set_error(error_message(&error)),
                }),
            );
        }
    }

    fn render(&mut self, props: &WorktreesFlowProps<'_>, buf: &mut Buffer, area: Rect) {
        let rows = self.list_rows(props, usize::from(area.width), usize::from(area.height), date::now_ms());
        let content = bordered(buf, area, &Border::round(THEME.border_active), 1);
        let mut column = Column::new();
        column.extend(rows);
        column.draw(buf, content);
        // A deletion asks in a dialog over the (faded) list it was started from.
        if let Some(state) = self.confirm.as_ref() {
            dim_area(buf, area);
            let options = self.options(state, props);
            let open_items: Vec<String> = state
                .entries
                .iter()
                .flat_map(|entry| open_items_of(entry, props.sessions))
                .collect();
            let pane = ConfirmPaneProps {
                state,
                options: &options,
                sessions: props.sessions,
                open_items: &open_items,
                working: self.working.as_deref(),
                loading: self.confirm_loading(state),
            };
            let (scroll, last) = self.confirm_scroll;
            let mut view = last.view(scroll);
            // Moving between deleting the worktree and deleting it with its branch shows the other check: open at
            // the size of the bigger, so it never resizes.
            let selected = options
                .get(state.index.min(options.len().saturating_sub(1)))
                .map(|option| option.kind);
            let other_kind = match selected {
                Some(ConfirmKind::DeleteBranch) => Some(ConfirmKind::Delete),
                Some(ConfirmKind::Delete) => Some(ConfirmKind::DeleteBranch),
                _ => None,
            };
            if let Some(other_index) = other_kind
                .filter(|_| !pane.loading)
                .and_then(|kind| options.iter().position(|option| option.kind == kind))
            {
                let other_state = DeleteConfirm {
                    index: other_index,
                    ..state.clone()
                };
                let other = ConfirmPaneProps {
                    state: &other_state,
                    ..pane
                };
                view = dialog_view_fitting(area, &|width| confirm_pane_rows(&other, usize::from(width)), view);
            }
            self.confirm_scroll.1 = render_confirm_pane(&pane, buf, area, view);
        }
    }

    /// TS: WorktreesFlow.hint: the manager's footer.
    fn hint(&self, props: &WorktreesFlowProps<'_>, width: usize) -> String {
        let part = |text: &str, short: Option<&str>, drop: Option<i32>| HintPart {
            text: text.to_owned(),
            short: short.map(str::to_owned),
            drop,
        };
        if let Some(confirm) = self.confirm.as_ref() {
            if self.confirm_loading(confirm) {
                return fit_hint(&[HintPart::from("esc cancel")], width, HINT_SEPARATOR);
            }
            if confirm.override_.is_some() {
                return fit_hint(
                    &[HintPart::from("type DELETE then enter"), HintPart::from("esc back")],
                    width,
                    HINT_SEPARATOR,
                );
            }
            let mut parts = vec![HintPart::from("enter choose")];
            if confirm
                .entries
                .iter()
                .any(|entry| !open_items_of(entry, props.sessions).is_empty())
            {
                let text = if confirm.keep_open_items {
                    "space send note items"
                } else {
                    "space keep note items"
                };
                parts.push(part(text, Some("space note items"), None));
            }
            parts.push(HintPart::from("esc cancel"));
            return fit_hint(&parts, width, HINT_SEPARATOR);
        }
        let on_ready = self.view(props, date::now_ms()).on_ready();
        let parts = [
            if on_ready {
                part("x delete them all", Some("x delete all"), None)
            } else {
                HintPart::from("x delete")
            },
            part("enter its session", Some("enter session"), Some(1)),
            part("M merged", None, Some(2)),
            part("E open in editor", Some("E editor"), Some(1)),
            part("R refresh", None, Some(3)),
            part("esc back", None, Some(2)),
        ];
        fit_hint(&parts, width, HINT_SEPARATOR)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::event_loop::Update;
    use crate::app::test_support::app;
    use crate::keymap::{undocumented_in_topic, unknown_topics};
    use crate::primitives::row_text;
    use deckhand_client::live::LiveClientHandlers;
    use deckhand_core::key::Key;
    use serde_json::{Value, json};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::UnixStream;
    use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
    use tokio::sync::mpsc::UnboundedReceiver;

    fn session(
        id: &str,
        title: &str,
        status: SessionStatus,
        worktree: Option<&str>,
        note: Option<&str>,
    ) -> SessionRecord {
        let mut value = json!({
            "id": id,
            "title": title,
            "program": "claude",
            "command": "claude",
            "cwd": "/repo",
            "repoRoot": "/repo",
            "status": status.as_str(),
            "agentStatus": "idle",
            "createdAt": "2026-01-01T00:00:00.000Z",
            "updatedAt": "2026-01-01T00:00:00.000Z",
        });
        if let Some(id) = worktree {
            value["worktree"] = json!({"mode": "managed", "id": id});
            if let Some(text) = note {
                value["sharedNotes"] =
                    json!({"kind": "worktree", "id": id, "path": "/notes/w.md", "text": text, "revision": "1"});
            }
        }
        serde_json::from_value(value).unwrap_or_else(|error| panic!("{error}"))
    }

    fn inspection(safe: bool, reasons: &[&str]) -> Value {
        json!({"safe": safe, "reasons": reasons, "dirtyFiles": 0, "untrackedFiles": if safe { 0 } else { 1 }, "ignoredFiles": 0})
    }

    /// A merged, clean worktree with a running session (ready), a git-made one with an untracked file (in
    /// progress) and the main checkout.
    fn overview() -> Value {
        json!({
            "mainRoot": "/repo",
            "defaultBranch": "main",
            "entries": [
                {"path": "/repo", "branch": "main", "head": "a1", "isMain": true, "sessionIds": [], "runningIds": []},
                {"path": "/wt/shipped", "branch": "deckhand/shipped", "head": "b2", "isMain": false, "recordId": "w1",
                 "markers": {"mergedAt": "2026-01-01T00:00:00.000Z", "mergeDetected": "ancestor"},
                 "sessionIds": ["s1"], "runningIds": [], "aheadOfDefault": 1, "inspection": inspection(true, &[])},
                {"path": "/wt/scratch", "branch": "scratch", "head": "c3", "isMain": false,
                 "sessionIds": [], "runningIds": [], "aheadOfDefault": 0,
                 "inspection": inspection(false, &["1 untracked file(s)"])},
            ],
            "checkedAt": "2026-01-01T00:00:00.000Z",
        })
    }

    fn overview_record() -> WorktreeOverview {
        serde_json::from_value(overview()).unwrap_or_else(|error| panic!("{error}"))
    }

    fn sessions() -> Vec<SessionRecord> {
        vec![session(
            "s1",
            "shipped",
            SessionStatus::Running,
            Some("w1"),
            Some("# Notes\n- [ ] Follow up on perf\n"),
        )]
    }

    fn props<'a>(sessions: &'a [&'a SessionRecord]) -> WorktreesFlowProps<'a> {
        WorktreesFlowProps {
            client: None,
            cwd: "/repo",
            sessions,
            spinner_frame: "⠋",
        }
    }

    fn cx() -> (FlowCx<App, WorktreesFlow>, UnboundedReceiver<Update<App>>) {
        let (dispatch, rx) = crate::app::event_loop::Dispatch::channel();
        (FlowCx::new(dispatch, |app: &mut App| &mut app.flows.worktrees), rx)
    }

    fn effects(cx: &mut FlowCx<App, WorktreesFlow>) -> Vec<String> {
        cx.take_effects().iter().map(|effect| format!("{effect:?}")).collect()
    }

    fn named(key: Key) -> Input {
        Input::key("", key)
    }

    fn enter() -> Input {
        named(Key {
            return_: true,
            ..Key::NONE
        })
    }

    fn escape() -> Input {
        named(Key {
            escape: true,
            ..Key::NONE
        })
    }

    fn screen(buf: &Buffer) -> Vec<String> {
        let area = buf.area;
        (area.top()..area.bottom())
            .map(|y| {
                let mut line = String::new();
                let mut x = area.left();
                while x < area.right() {
                    let symbol = buf[(x, y)].symbol();
                    line.push_str(symbol);
                    x += u16::try_from(crate::primitives::string_width(symbol).max(1)).unwrap_or(1);
                }
                line.trim_end().to_owned()
            })
            .collect()
    }

    fn render(flow: &mut WorktreesFlow, props: &WorktreesFlowProps<'_>, width: u16, height: u16) -> String {
        let area = Rect::new(0, 0, width, height);
        let mut buf = Buffer::empty(area);
        flow.render(props, &mut buf, area);
        screen(&buf).join("\n")
    }

    /// The daemon side of a socket pair.
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

        async fn respond(&mut self, request: &Value, body: Value) {
            let mut message = json!({"type": "response", "requestId": request["requestId"]});
            if let (Some(message), Some(body)) = (message.as_object_mut(), body.as_object()) {
                message.extend(body.clone());
            }
            self.writer
                .write_all(format!("{message}\n").as_bytes())
                .await
                .unwrap_or_else(|error| panic!("{error}"));
        }
    }

    /// An app for `/repo` connected to a fake daemon, with the given sessions.
    fn connected(sessions: Vec<SessionRecord>) -> (App, UnboundedReceiver<Update<App>>, Daemon) {
        let (mut app, rx) = app(crate::app::state::AppProps::default());
        let (socket, peer) = UnixStream::pair().unwrap_or_else(|error| panic!("{error}"));
        app.connection.client = Some(Rc::new(LiveClient::new(socket, LiveClientHandlers::default())));
        app.data.repo_sessions = sessions;
        let (read, writer) = peer.into_split();
        (
            app,
            rx,
            Daemon {
                lines: BufReader::new(read),
                writer,
            },
        )
    }

    async fn pump(app: &mut App, rx: &mut UnboundedReceiver<Update<App>>) {
        let update = rx.recv().await.unwrap_or_else(|| panic!("no update"));
        update(app);
    }

    fn press(app: &mut App, input: &Input) {
        app.worktrees_flow_call(|flow, props, cx| flow.handle_input(props, input, cx));
    }

    fn shown(app: &mut App, width: u16, height: u16) -> String {
        app.worktrees_flow_call(|flow, props, _cx| render(flow, props, width, height))
    }

    fn run(test: impl std::future::Future<Output = ()>) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap_or_else(|error| panic!("{error}"));
        tokio::task::LocalSet::new().block_on(&runtime, test);
    }

    #[test]
    fn checked_ago_says_just_now_under_a_minute() {
        assert_eq!(checked_ago(0), "just now");
        assert_eq!(checked_ago(59_000), "just now");
        assert_eq!(checked_ago(3 * 60_000), "3m ago");
        assert_eq!(checked_ago_js(f64::NAN), "just now");
    }

    #[test]
    fn keys_are_in_the_worktrees_topic() {
        assert!(undocumented_in_topic(KEYMAP, "Worktrees").is_empty());
        assert!(unknown_topics(KEYMAP).is_empty());
    }

    #[test]
    fn list_lines_mark_the_selection_and_groups() {
        let records = sessions();
        let refs: Vec<&SessionRecord> = records.iter().collect();
        let overview = overview_record();
        let rows = worktree_rows(Some(&overview), &refs, date::now_ms());
        let heading = row_text(&worktree_line(&rows[0], true, 60, &refs, Some("main"), 0));
        assert_eq!(
            heading,
            js_pad_end("› Merged · safe to delete · 1  (x deletes them all)", 60, " ")
        );
        let plain = row_text(&worktree_line(&rows[0], false, 50, &refs, Some("main"), 0));
        assert_eq!(plain, "  Merged · safe to delete · 1");
        let shipped = worktree_line(&rows[1], false, 50, &refs, Some("main"), 0);
        let text = row_text(&shipped);
        assert!(text.starts_with("  ✓ deckhand/shipped"), "{text}");
        assert_eq!(text, "  ✓ deckhand/shipped     1 commit · ☐1 · 1 sessio…");
        assert_eq!(js_len(&text), 50);
        assert_eq!(shipped[1].style.color, Some(THEME.success));
        let selected = worktree_line(&rows[1], true, 50, &refs, Some("main"), 0);
        assert!(row_text(&selected).starts_with("› ✓"));
        assert_eq!(selected[1].style.color, Some(THEME.active));
        assert_eq!(selected[1].style.inverse, Some(true));
    }

    #[test]
    fn details_say_why_it_can_go() {
        let records = sessions();
        let refs: Vec<&SessionRecord> = records.iter().collect();
        let overview = overview_record();
        let rows = worktree_rows(Some(&overview), &refs, 0);
        let texts = |row: &WorktreeRow| -> Vec<String> {
            detail_lines(Some(row), &rows, &refs, Some("main"), "⠋", 0, 80)
                .iter()
                .map(|row| row_text(row))
                .collect()
        };
        assert_eq!(
            texts(&rows[1]),
            [
                "deckhand/shipped · merged into main",
                "/wt/shipped · 1 commit not in main",
                "● shipped",
                "Clean: deleting it and its branch loses nothing",
            ]
        );
        let heading = texts(&rows[0]);
        assert_eq!(
            heading[0],
            "x deletes this worktree and their branches (after one confirmation)"
        );
        assert_eq!(heading[1], "deckhand/shipped");
        let scratch = texts(&rows[3]);
        assert_eq!(scratch[0], "scratch · not merged");
        assert_eq!(scratch[2], "Not created or attached by Deckhand");
        assert_eq!(scratch[3], "Deleting it would lose: 1 untracked file(s)");
        assert!(detail_lines(None, &rows, &refs, None, "⠋", 0, 80).is_empty());
    }

    #[test]
    fn the_deletion_confirmation_shows_its_choices_once_both_checks_are_in() {
        let records = sessions();
        let refs: Vec<&SessionRecord> = records.iter().collect();
        let props = props(&refs);
        let entry = overview_record().entries[2].clone();
        let mut flow = WorktreesFlow {
            overview: Some(overview_record()),
            confirm: Some(DeleteConfirm {
                entries: vec![entry],
                bulk: false,
                index: 0,
                checks: ConfirmChecks::default(),
                override_: None,
                keep_open_items: false,
            }),
            ..WorktreesFlow::default()
        };
        let (mut cx, _rx) = cx();
        // Checking: no choices, only Esc; Enter says why.
        let text = render(&mut flow, &props, 98, 31);
        assert!(text.contains("Checking what it would lose…"), "{text}");
        assert!(!text.contains("Delete the worktree"), "{text}");
        assert_eq!(flow.hint(&props, 98), "esc cancel");
        flow.handle_input(&props, &Input::text("j"), &mut cx);
        flow.handle_input(&props, &enter(), &mut cx);
        assert_eq!(flow.confirm.as_ref().map(|state| state.index), Some(0));
        assert_eq!(
            effects(&mut cx),
            ["SetStatus(Some(\"Still checking what it would lose…\"))"]
        );
        // The worktree's answer alone is not enough while its branch could go too.
        let mut branch = failed_check("1 untracked file".to_owned());
        branch
            .inspection
            .inspection
            .reasons
            .extend(["3 unpublished commits", "the branch is not merged", "a stash"].map(str::to_owned));
        if let Some(state) = flow.confirm.as_mut() {
            state.checks.worktree = Some(failed_check("1 untracked file".to_owned()));
        }
        assert!(flow.confirm.as_ref().is_some_and(|state| flow.confirm_loading(state)));
        if let Some(state) = flow.confirm.as_mut() {
            state.checks.branch = Some(branch);
        }
        // Both in: the choices, and one size whichever check the selection shows.
        let corners = |text: &str| -> (Option<usize>, Option<usize>) {
            let rows: Vec<&str> = text.lines().collect();
            (
                rows.iter().rposition(|row| row.contains('╭')),
                rows.iter().position(|row| row.contains('╯')),
            )
        };
        let first = render(&mut flow, &props, 98, 31);
        assert!(first.contains("Delete the worktree, keep its branch"), "{first}");
        flow.handle_input(&props, &Input::text("j"), &mut cx);
        let second = render(&mut flow, &props, 98, 31);
        assert!(second.contains("3 unpublished commits"), "{second}");
        assert_eq!(corners(&second), corners(&first));
        // Esc still cancels.
        flow.handle_input(&props, &escape(), &mut cx);
        assert!(flow.confirm.is_none());
    }

    #[test]
    fn options_offer_the_branch_first_when_merged_and_name_the_stops() {
        let records = sessions();
        let refs: Vec<&SessionRecord> = records.iter().collect();
        let props = props(&refs);
        let flow = WorktreesFlow {
            overview: Some(overview_record()),
            ..WorktreesFlow::default()
        };
        let entries = overview_record().entries;
        let confirm = |entries: Vec<WorktreeOverviewEntry>, bulk: bool| DeleteConfirm {
            entries,
            bulk,
            index: 0,
            checks: ConfirmChecks::default(),
            override_: None,
            keep_open_items: false,
        };
        let labels = |state: &DeleteConfirm| -> Vec<String> {
            flow.options(state, &props)
                .into_iter()
                .map(|option| option.label)
                .collect()
        };
        assert_eq!(
            labels(&confirm(vec![entries[1].clone()], false)),
            [
                "Stop 1 session, delete the worktree and branch deckhand/shipped",
                "Stop 1 session, delete the worktree, keep its branch",
                "Cancel"
            ]
        );
        assert_eq!(
            labels(&confirm(vec![entries[2].clone()], false)),
            [
                "Delete the worktree, keep its branch",
                "Delete the worktree and branch scratch",
                "Cancel"
            ]
        );
        assert_eq!(
            labels(&confirm(vec![entries[1].clone()], true)),
            [
                "Stop 1 session, delete 1 worktree and their branches",
                "Stop 1 session, delete 1 worktree, keep the branches",
                "Cancel"
            ]
        );
        let mut main = entries[0].clone();
        main.is_main = false;
        assert_eq!(
            labels(&confirm(vec![main.clone()], false)),
            ["Delete the worktree, keep its branch", "Cancel"]
        );
        main.missing = Some(WorktreeMissing::Prunable);
        assert_eq!(
            labels(&confirm(vec![main.clone()], false)),
            ["Prune it: Git forgets the worktree (its branch stays)", "Cancel"]
        );
        main.missing = Some(WorktreeMissing::Unlisted);
        assert_eq!(
            labels(&confirm(vec![main], false)),
            ["Forget it: Deckhand's record only, nothing on disk", "Cancel"]
        );
    }

    #[test]
    fn keys_move_refuse_and_go_to_sessions() {
        let records = sessions();
        let refs: Vec<&SessionRecord> = records.iter().collect();
        let props = props(&refs);
        let (mut cx, _rx) = cx();
        let mut flow = WorktreesFlow {
            overview: Some(overview_record()),
            ..WorktreesFlow::default()
        };
        // Rows: ready heading, shipped, progress heading, scratch, main heading, main.
        let current = |flow: &WorktreesFlow| flow.view(&props, 0).current;
        assert_eq!(current(&flow), Some(0));
        assert_eq!(
            flow.hint(&props, 200),
            "x delete them all • enter its session • M merged • E open in editor • R refresh • esc back"
        );
        flow.handle_input(&props, &Input::text("j"), &mut cx);
        assert_eq!(current(&flow), Some(1));
        flow.handle_input(&props, &Input::text("o"), &mut cx);
        assert_eq!(effects(&mut cx), ["GoTo(\"s1\")"]);
        flow.handle_input(&props, &Input::text("j"), &mut cx);
        assert_eq!(current(&flow), Some(3));
        flow.handle_input(&props, &enter(), &mut cx);
        assert_eq!(
            effects(&mut cx),
            ["SetStatus(Some(\"No session in it: x deletes it, E opens it in your editor\"))"]
        );
        flow.handle_input(&props, &Input::text("M"), &mut cx);
        assert_eq!(
            effects(&mut cx),
            ["SetStatus(Some(\"Only worktrees Deckhand tracks have a merged marker\"))"]
        );
        flow.handle_input(&props, &Input::text("G"), &mut cx);
        assert_eq!(current(&flow), Some(5));
        flow.handle_input(&props, &Input::text("x"), &mut cx);
        assert_eq!(
            effects(&mut cx),
            ["SetStatus(Some(\"The main checkout is never deleted\"))"]
        );
        flow.handle_input(&props, &Input::text("j"), &mut cx);
        assert_eq!(current(&flow), Some(5));
        let page_up = named(Key {
            page_up: true,
            ..Key::NONE
        });
        flow.handle_input(&props, &page_up, &mut cx);
        assert_eq!(current(&flow), Some(0));
        flow.handle_input(&props, &Input::text("g"), &mut cx);
        assert_eq!(current(&flow), Some(0));
        // Without a client: R asks for one; x on the heading opens the bulk confirmation (nothing to check).
        flow.handle_input(&props, &Input::text("R"), &mut cx);
        assert_eq!(
            effects(&mut cx),
            [
                "SetError(Some(\"still connecting to daemon\"))",
                "SetStatus(Some(\"Checking the worktrees again\"))"
            ]
        );
        flow.handle_input(&props, &Input::text("x"), &mut cx);
        assert!(flow.confirm.as_ref().is_some_and(|confirm| confirm.bulk));
        assert_eq!(
            flow.hint(&props, 200),
            "enter choose • space keep note items • esc cancel"
        );
        flow.handle_input(&props, &Input::text(" "), &mut cx);
        assert_eq!(
            flow.hint(&props, 200),
            "enter choose • space send note items • esc cancel"
        );
        flow.handle_input(&props, &Input::text("k"), &mut cx);
        assert_eq!(flow.confirm.as_ref().map(|confirm| confirm.index), Some(2));
        flow.handle_input(&props, &enter(), &mut cx);
        assert!(flow.confirm.is_none());
        flow.handle_input(&props, &escape(), &mut cx);
        assert_eq!(effects(&mut cx), ["Exit"]);
    }

    #[test]
    fn empty_list_explains_how_to_make_one() {
        let refs: Vec<&SessionRecord> = Vec::new();
        let props = props(&refs);
        let mut overview = overview_record();
        overview.entries.truncate(1);
        let mut flow = WorktreesFlow {
            overview: Some(overview),
            ..WorktreesFlow::default()
        };
        let text = render(&mut flow, &props, 80, 20);
        assert!(text.contains("⎇ Worktrees · repo"), "{text}");
        assert!(text.contains("0 worktrees"), "{text}");
        assert!(
            text.contains("No other worktrees: esc, then n and tab creates one with a new session"),
            "{text}"
        );
        assert!(text.contains("main · the main checkout"), "{text}");
        assert!(text.contains("No sessions"), "{text}");
    }

    #[test]
    fn w_loads_selects_and_deletes_the_ready_group() {
        run(async {
            let (mut app, mut rx, mut daemon) = connected(sessions());
            app.worktrees_flow_call(|flow, props, cx| flow.open(Some("/wt/shipped"), props, cx));
            assert!(app.flows.worktrees.loading);
            let request = daemon.next().await;
            assert_eq!(request["type"], "worktree-overview");
            assert_eq!(request["cwd"], "/repo");
            daemon.respond(&request, json!({"ok": true, "data": overview()})).await;
            pump(&mut app, &mut rx).await;
            assert!(!app.flows.worktrees.loading);
            assert_eq!(app.flows.worktrees.selected, (Some("wt:/wt/shipped".to_owned()), 0));

            let screen = shown(&mut app, 90, 24);
            assert!(screen.contains("2 worktrees · 1 ready to delete"), "{screen}");
            assert!(screen.contains("Compared with main · checked"), "{screen}");
            assert!(screen.contains("Merged · safe to delete · 1"), "{screen}");
            assert!(screen.contains("› ✓ deckhand/shipped"), "{screen}");
            assert!(screen.contains("Main checkout"), "{screen}");
            assert!(
                screen.contains("Clean: deleting it and its branch loses nothing"),
                "{screen}"
            );

            press(&mut app, &Input::text("k"));
            let screen = shown(&mut app, 90, 24);
            assert!(screen.contains("x deletes them all"), "{screen}");
            press(&mut app, &Input::text("x"));
            let screen = shown(&mut app, 90, 24);
            assert!(screen.contains("Delete 1 merged worktree?"), "{screen}");
            assert!(
                screen.contains("Stops the session running there first: shipped"),
                "{screen}"
            );
            assert!(
                screen.contains("Stop 1 session, delete 1 worktree and their branches"),
                "{screen}"
            );
            assert!(
                screen.contains("☑ 1 open note item → backlog first: Follow up on perf"),
                "{screen}"
            );
            press(&mut app, &Input::text(" "));
            let screen = shown(&mut app, 90, 24);
            assert!(
                screen.contains("☐ 1 open note item stay in the (read-only) note"),
                "{screen}"
            );
            press(&mut app, &Input::text(" "));

            press(&mut app, &enter());
            assert_eq!(
                app.flows.worktrees.working.as_deref(),
                Some("Deleting deckhand/shipped… 1/1")
            );
            let request = daemon.next().await;
            assert_eq!(request["type"], "delete-worktree");
            let object = request.as_object().cloned().unwrap_or_default();
            let keys: Vec<&str> = object.keys().map(String::as_str).collect();
            assert_eq!(
                keys,
                [
                    "type",
                    "requestId",
                    "cwd",
                    "path",
                    "branch",
                    "deleteBranch",
                    "stopSessions",
                    "sendOpenItems"
                ]
            );
            assert_eq!(request["path"], "/wt/shipped");
            assert_eq!(request["deleteBranch"], true);
            assert_eq!(request["sendOpenItems"], true);
            // Keys wait while it works.
            press(&mut app, &escape());
            assert!(app.flows.worktrees.confirm.is_some());
            let screen = shown(&mut app, 90, 24);
            assert!(screen.contains("Deleting deckhand/shipped… 1/1"), "{screen}");
            let result = json!({"removed": "deleted", "branchDeleted": "deckhand/shipped", "stopped": 1, "archived": 1, "sentItems": 1});
            daemon.respond(&request, json!({"ok": true, "data": result})).await;
            pump(&mut app, &mut rx).await;
            assert_eq!(
                app.messages.status_message.as_deref(),
                Some("Deleted 1 worktree and 1 branch · stopped 1 session · 1 open note item to the backlog")
            );
            assert!(app.flows.worktrees.confirm.is_none());
            assert!(app.flows.worktrees.working.is_none());
            assert_eq!(daemon.next().await["type"], "worktree-overview");
        });
    }

    #[test]
    fn data_loss_needs_delete_typed() {
        run(async {
            let (mut app, mut rx, mut daemon) = connected(sessions());
            app.worktrees_flow_call(|flow, props, cx| flow.open(None, props, cx));
            let request = daemon.next().await;
            daemon.respond(&request, json!({"ok": true, "data": overview()})).await;
            pump(&mut app, &mut rx).await;
            press(&mut app, &Input::text("j"));
            press(&mut app, &Input::text("j"));
            press(&mut app, &Input::text("x"));
            let screen = shown(&mut app, 90, 30);
            assert!(screen.contains("Checking what it would lose…"), "{screen}");
            // Enter before the check is back only says so.
            press(&mut app, &enter());
            assert_eq!(
                app.messages.status_message.as_deref(),
                Some("Still checking what it would lose…")
            );
            let alone = daemon.next().await;
            assert_eq!(alone["type"], "inspect-worktree");
            assert_eq!(alone["deleteBranch"], false);
            let with_branch = daemon.next().await;
            assert_eq!(with_branch["deleteBranch"], true);
            let mut lossy = inspection(false, &["1 untracked file(s)"]);
            lossy["files"] = json!([{"path": "notes.txt", "state": "untracked"}]);
            lossy["structuralBlockers"] = json!([]);
            lossy["running"] = json!([]);
            daemon.respond(&alone, json!({"ok": true, "data": lossy})).await;
            daemon
                .respond(&with_branch, json!({"ok": false, "error": "inspect failed"}))
                .await;
            pump(&mut app, &mut rx).await;
            pump(&mut app, &mut rx).await;
            let checks = app
                .flows
                .worktrees
                .confirm
                .as_ref()
                .map(|confirm| confirm.checks.clone())
                .unwrap_or_default();
            assert_eq!(
                checks.branch.map(|check| check.inspection.inspection.reasons),
                Some(vec!["inspect failed".to_owned()])
            );
            let screen = shown(&mut app, 90, 30);
            assert!(screen.contains("Delete scratch?"), "{screen}");
            assert!(
                screen.contains("Deleting it would lose (typing DELETE overrides):"),
                "{screen}"
            );
            assert!(screen.contains("? notes.txt"), "{screen}");
            assert!(screen.contains("Delete the worktree, keep its branch"), "{screen}");

            press(&mut app, &enter());
            let screen = shown(&mut app, 90, 30);
            assert!(screen.contains("Delete scratch anyway?"), "{screen}");
            assert!(screen.contains("Type DELETE then enter:"), "{screen}");
            assert_eq!(
                app.worktrees_flow_call(|flow, props, _cx| flow.hint(props, 100)),
                "type DELETE then enter • esc back"
            );
            press(&mut app, &Input::text("DELE"));
            press(&mut app, &Input::text("1"));
            press(&mut app, &Input::text("TX"));
            press(
                &mut app,
                &named(Key {
                    backspace: true,
                    ..Key::NONE
                }),
            );
            press(&mut app, &enter());
            let draft = |app: &App| {
                app.flows
                    .worktrees
                    .confirm
                    .as_ref()
                    .and_then(|confirm| confirm.override_.clone())
                    .map(|override_| override_.draft)
            };
            assert_eq!(draft(&app).as_deref(), Some("DELET"));
            assert!(app.flows.worktrees.working.is_none());
            press(&mut app, &Input::text("E"));
            press(&mut app, &enter());
            assert_eq!(app.flows.worktrees.working.as_deref(), Some("Deleting scratch…"));
            let request = daemon.next().await;
            assert_eq!(request["type"], "delete-worktree");
            assert_eq!(request["branch"], "scratch");
            assert_eq!(request["deleteBranch"], false);
            assert_eq!(request["allowDataLoss"], true);
            assert_eq!(request["sendOpenItems"], false);
            daemon
                .respond(
                    &request,
                    json!({"ok": true, "data": {"removed": "deleted", "stopped": 0, "archived": 0}}),
                )
                .await;
            pump(&mut app, &mut rx).await;
            assert_eq!(app.messages.status_message.as_deref(), Some("Deleted scratch"));
            assert!(app.flows.worktrees.confirm.is_none());
        });
    }

    #[test]
    fn failures_are_reported_and_the_list_reloads() {
        run(async {
            let (mut app, mut rx, mut daemon) = connected(Vec::new());
            app.worktrees_flow_call(|flow, props, cx| flow.open(None, props, cx));
            let request = daemon.next().await;
            daemon
                .respond(&request, json!({"ok": false, "error": "not a git repository"}))
                .await;
            pump(&mut app, &mut rx).await;
            assert_eq!(app.messages.error.as_deref(), Some("not a git repository"));
            assert!(!app.flows.worktrees.loading);
            app.flows.worktrees.overview = Some(overview_record());
            let entry = overview_record().entries[2].clone();
            app.worktrees_flow_call(|flow, props, cx| flow.delete_one(&entry, true, false, false, props, cx));
            let request = daemon.next().await;
            daemon
                .respond(&request, json!({"ok": false, "error": "worktree is dirty"}))
                .await;
            pump(&mut app, &mut rx).await;
            assert_eq!(app.messages.error.as_deref(), Some("worktree is dirty"));
            assert!(app.flows.worktrees.working.is_none());
            assert_eq!(daemon.next().await["type"], "worktree-overview");
        });
    }
}
