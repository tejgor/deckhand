//! Notes as pure data: identities, revisions, Markdown checklists and the Notes tab's layout. The daemon keeps the
//! files (`deckhand_sys::notes_store`); the UI edits them (notes flow) and renders them (notes pane).
//!
//! Offsets: [`ChecklistItem::prefix`] and [`ChecklistItem::box_at`] are byte offsets into the line, which equal the
//! TS UTF-16 offsets (the marker is ASCII). Editor states use byte cursors (see `text_editor`).

use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;
use sha2::{Digest, Sha256};

use crate::display::{DISPLAY_CONTROL_PATTERN, truncate};
use crate::jsstr::{js_len, js_trim, path};
use crate::text_editor::{EditorState, line_end, line_start, wrap_rows};

/// TS: notes.ts MAX_NOTES_CHARS (UTF-16 units).
pub const MAX_NOTES_CHARS: usize = 50_000;

/// TS: notes.ts MAX_NOTES_LABEL.
pub const MAX_NOTES_LABEL: &str = "50 000 characters";

/// The first 16 hex digits of the sha256 of `text` (as UTF-8, like Node's `hash.update(string)`).
fn short_sha256(text: &str) -> String {
    Sha256::digest(text.as_bytes())
        .iter()
        .take(8)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// TS: notes.ts noteRevision (first 16 hex of the text's sha256). A missing file is the empty text; saves name the
/// revision they edited.
pub fn note_revision(text: &str) -> String {
    short_sha256(text)
}

/// TS: notes.ts repoNoteId (first 16 hex of the resolved root's sha256): the file-name-safe key of the main
/// checkout's (repository) note.
pub fn repo_note_id(root: &str) -> String {
    short_sha256(&path::resolve(root))
}

// ── Checklists ────────────────────────────────────────────────────────────────────────────────────────────────

static CHECKLIST: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^([ \t]*)([-*+])([ \t]+)\[([ xX])\](?:[ \t]|$)").expect("CHECKLIST is a valid regex")
});
static BULLET: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^([ \t]*)([-*+])[ \t]+").expect("BULLET is a valid regex"));
static LINKED_TASK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"<!--[ \t]*dh:t=([0-9a-z]+)").expect("LINKED_TASK is a valid regex"));
/// A line sent to Tasks (`- ↗ <title> <!-- dh:t=<id> -->`).
static LINK_LINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^([ \t]*)[-*+][ \t]+↗[ \t]+(.*?)[ \t]*(?:<!--[ \t]*dh:[^>]*?-->)?[ \t]*$")
        .expect("LINK_LINE is a valid regex")
});
static HEADING: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^#{1,6}[ \t]").expect("HEADING is a valid regex"));

/// The leading spaces and tabs of `line` (`/^[ \t]*/`).
fn leading_blanks(line: &str) -> &str {
    &line[..line.len() - line.trim_start_matches([' ', '\t']).len()]
}

/// `text.slice(start, end)` on byte offsets: clamped to the text, rounded down to char boundaries.
fn slice(text: &str, start: usize, end: usize) -> &str {
    let floor = |index: usize| {
        let mut index = index.min(text.len());
        while !text.is_char_boundary(index) {
            index -= 1;
        }
        index
    };
    let (start, end) = (floor(start), floor(end));
    if start >= end { "" } else { &text[start..end] }
}

/// `text.slice(at)` on a byte offset.
fn slice_from(text: &str, at: usize) -> &str {
    slice(text, at, text.len())
}

/// TS: notes.ts ChecklistItem (`- [ ] item`; `prefix`: the marker's length with its trailing space; `box`: the
/// offset of the mark inside the brackets).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChecklistItem {
    pub indent: String,
    pub bullet: String,
    pub checked: bool,
    pub prefix: usize,
    pub text: String,
    #[serde(rename = "box")]
    pub box_at: usize,
}

/// TS: notes.ts parseChecklistLine: `- [ ] item` / `* [x] item` / `+ [X] item`, indented or not.
pub fn parse_checklist_line(line: &str) -> Option<ChecklistItem> {
    let captures = CHECKLIST.captures(line)?;
    let group = |index: usize| captures.get(index).map_or("", |found| found.as_str());
    let whole = group(0).len();
    let (indent, bullet, gap) = (group(1), group(2), group(3));
    Some(ChecklistItem {
        indent: indent.to_owned(),
        bullet: bullet.to_owned(),
        checked: group(4) != " ",
        prefix: whole,
        text: slice_from(line, whole).to_owned(),
        box_at: indent.len() + bullet.len() + gap.len() + 1,
    })
}

/// TS: notes.ts checklistCounts result.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChecklistCounts {
    pub open: usize,
    pub done: usize,
}

/// TS: notes.ts checklistCounts.
pub fn checklist_counts(text: Option<&str>) -> ChecklistCounts {
    let mut counts = ChecklistCounts::default();
    for item in text.unwrap_or("").split('\n').filter_map(parse_checklist_line) {
        if item.checked {
            counts.done += 1;
        } else {
            counts.open += 1;
        }
    }
    counts
}

/// `text` with the checklist mark at byte `at` flipped (`checked`: what it is now).
fn flip_mark(text: &str, at: usize, checked: bool) -> String {
    format!(
        "{}{}{}",
        slice(text, 0, at),
        if checked { ' ' } else { 'x' },
        slice_from(text, at + 1)
    )
}

/// TS: notes.ts toggleChecklistLine (`line` 0-based; `None` when that line is not an item).
pub fn toggle_checklist_line(text: &str, line: usize) -> Option<String> {
    let mut lines: Vec<String> = text.split('\n').map(str::to_owned).collect();
    let current = lines.get_mut(line)?;
    let item = parse_checklist_line(current)?;
    *current = flip_mark(current, item.box_at, item.checked);
    Some(lines.join("\n"))
}

/// TS: notes.ts TaskLinkState (where the task a note's `↗` line links to is).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TaskLinkState {
    #[serde(rename = "open")]
    Open,
    #[serde(rename = "done")]
    Done,
    #[serde(rename = "gone")]
    Gone,
}

/// TS: notes.ts TaskLinkLookup.
pub type TaskLinkLookup<'a> = &'a dyn Fn(&str) -> TaskLinkState;

/// TS: notes.ts taskLinkSuffix (`· in Tasks` without a state: nothing is claimed before the list is loaded).
pub fn task_link_suffix(state: Option<TaskLinkState>) -> &'static str {
    match state {
        Some(TaskLinkState::Done) => "· done",
        Some(TaskLinkState::Gone) => "· not in Tasks",
        Some(TaskLinkState::Open) | None => "· in Tasks",
    }
}

/// TS: notes.ts linkedTaskId (the `<!-- dh:t=<id> -->` task ID of a `↗` line's bookkeeping).
pub fn linked_task_id(line: &str) -> Option<&str> {
    LINKED_TASK
        .captures(line)
        .and_then(|captures| captures.get(1))
        .map(|found| found.as_str())
}

/// An editor state as TS returns them (`{text, cursor}`).
fn edited(text: String, cursor: usize) -> EditorState {
    EditorState {
        text,
        cursor,
        ..EditorState::default()
    }
}

/// TS: notes.ts toggleChecklist (Ctrl+X): checks/unchecks the cursor line's item; a line without one becomes an open
/// item (a plain bullet keeps its bullet) and the cursor stays on the same text.
pub fn toggle_checklist(state: &EditorState) -> EditorState {
    let (text, cursor) = (state.text.as_str(), state.cursor);
    let start = line_start(text, cursor);
    let line = slice(text, start, line_end(text, cursor));
    if let Some(item) = parse_checklist_line(line) {
        return edited(flip_mark(text, start + item.box_at, item.checked), cursor);
    }
    let bullet = BULLET.find(line);
    let insert_at = start + bullet.map_or_else(|| leading_blanks(line).len(), |found| found.end());
    let inserted = if bullet.is_some() { "[ ] " } else { "- [ ] " };
    edited(
        format!("{}{inserted}{}", slice(text, 0, insert_at), slice_from(text, insert_at)),
        if cursor >= insert_at {
            cursor + inserted.len()
        } else {
            cursor
        },
    )
}

/// TS: notes.ts insertChecklistItem (Ctrl+T): a new open item below the cursor's line, with that line's indent (and
/// bullet, for an item).
pub fn insert_checklist_item(state: &EditorState) -> EditorState {
    let (text, cursor) = (state.text.as_str(), state.cursor);
    let (start, end) = (line_start(text, cursor), line_end(text, cursor));
    let line = slice(text, start, end);
    let item = parse_checklist_line(line);
    let indent = item
        .as_ref()
        .map_or_else(|| leading_blanks(line), |item| item.indent.as_str());
    // An empty line takes the item itself.
    if js_trim(line).is_empty() {
        return edited(
            format!("{}{indent}- [ ] {}", slice(text, 0, start), slice_from(text, end)),
            start + indent.len() + 6,
        );
    }
    let bullet = item.as_ref().map_or("-", |item| item.bullet.as_str());
    let inserted = format!("\n{indent}{bullet} [ ] ");
    edited(
        format!("{}{inserted}{}", slice(text, 0, end), slice_from(text, end)),
        end + inserted.len(),
    )
}

/// TS: notes.ts continueChecklist (Enter on an item, after its marker): a new open item with the rest of the line;
/// on an empty item the list ends (the marker is removed). `None`: a plain newline.
pub fn continue_checklist(state: &EditorState) -> Option<EditorState> {
    let (text, cursor) = (state.text.as_str(), state.cursor);
    let (start, end) = (line_start(text, cursor), line_end(text, cursor));
    let item = parse_checklist_line(slice(text, start, end))?;
    if cursor.saturating_sub(start) < item.prefix {
        return None;
    }
    if js_trim(&item.text).is_empty() {
        return Some(edited(
            format!("{}{}", slice(text, 0, start), slice_from(text, end)),
            start,
        ));
    }
    let inserted = format!("\n{}{} [ ] ", item.indent, item.bullet);
    Some(edited(
        format!("{}{inserted}{}", slice(text, 0, cursor), slice_from(text, cursor)),
        cursor + inserted.len(),
    ))
}

// ── Read mode ─────────────────────────────────────────────────────────────────────────────────────────────────

/// TS: notes.ts NoteRow kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum NoteRowKind {
    #[serde(rename = "open")]
    Open,
    #[serde(rename = "done")]
    Done,
    #[serde(rename = "heading")]
    Heading,
    #[serde(rename = "more")]
    More,
    #[serde(rename = "link")]
    Link,
}

/// TS: notes.ts NoteRow (one row of a note as the Notes tab shows it outside editing).
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NoteRow {
    pub text: String,
    pub kind: Option<NoteRowKind>,
}

fn row(text: String, kind: Option<NoteRowKind>) -> NoteRow {
    NoteRow { text, kind }
}

/// TS: notes.ts visibleText (tabs as spaces, other controls and U+2028/2029 as `?`).
pub fn visible_text(text: &str) -> String {
    let spaced = text.replace('\t', " ");
    DISPLAY_CONTROL_PATTERN
        .replace_all(&spaced, "?")
        .replace(['\u{2028}', '\u{2029}'], "?")
}

/// The rows of `text` word-wrapped at `width`.
fn wrapped(text: &str, width: usize) -> Vec<String> {
    wrap_rows(text, width, false)
        .into_iter()
        .map(|wrap| slice(text, wrap.start, wrap.end).to_owned())
        .collect()
}

/// TS: notes.ts noteReadRows (word-wrapped at `width`; checklist items as `☐ item` / `☑ item` with a hanging indent,
/// headings marked, `↗` lines without their bookkeeping and saying where their task is).
pub fn note_read_rows(text: &str, width: usize, links: Option<TaskLinkLookup<'_>>) -> Vec<NoteRow> {
    let columns = width.max(1);
    let mut rows = Vec::new();
    for raw in visible_text(text).split('\n') {
        // A line sent to Tasks (`- ↗ <title> <!-- dh:t=<id> -->`): shown without its bookkeeping.
        if let Some(link) = LINK_LINE.captures(raw) {
            let group = |index: usize| link.get(index).map_or("", |found| found.as_str());
            let state = linked_task_id(raw).and_then(|id| links.map(|lookup| lookup(id)));
            let line = format!("{}↗ {} {}", group(1), group(2), task_link_suffix(state));
            rows.extend(
                wrapped(&line, columns)
                    .into_iter()
                    .map(|text| row(text, Some(NoteRowKind::Link))),
            );
            continue;
        }
        if let Some(item) = parse_checklist_line(raw) {
            let marker = format!("{}{} ", item.indent, if item.checked { '☑' } else { '☐' });
            let kind = Some(if item.checked {
                NoteRowKind::Done
            } else {
                NoteRowKind::Open
            });
            // Continuation rows hang under the item's text while that leaves a useful width.
            let marker_len = js_len(&marker);
            let hang = if columns >= marker_len + 8 { marker_len } else { 0 };
            if hang > 0 {
                for (index, line) in wrapped(&item.text, columns - hang).into_iter().enumerate() {
                    let lead = if index > 0 { " ".repeat(hang) } else { marker.clone() };
                    rows.push(row(format!("{lead}{line}"), kind));
                }
            } else {
                let body = format!("{marker}{}", item.text);
                rows.extend(wrapped(&body, columns).into_iter().map(|line| row(line, kind)));
            }
            continue;
        }
        let kind = HEADING.is_match(raw).then_some(NoteRowKind::Heading);
        rows.extend(wrapped(raw, columns).into_iter().map(|line| row(line, kind)));
    }
    rows
}

/// TS: notes.ts fitReadRows (rows within `height` from the top; when they do not fit, the last row says how many more
/// there are).
pub fn fit_read_rows(rows: Vec<NoteRow>, height: usize) -> Vec<NoteRow> {
    if rows.len() <= height {
        return rows;
    }
    if height == 0 {
        return Vec::new();
    }
    let total = rows.len();
    let mut shown: Vec<NoteRow> = rows.into_iter().take(height - 1).collect();
    let more = total - shown.len();
    shown.push(row(
        format!("+{more} more line{}", if more == 1 { "" } else { "s" }),
        Some(NoteRowKind::More),
    ));
    shown
}

// ── Layout ────────────────────────────────────────────────────────────────────────────────────────────────────

/// TS: notes.ts MIN_SECTION_ROWS (the smallest body a section with more content than fits gets, unless the pane
/// itself is smaller).
pub const MIN_SECTION_ROWS: usize = 3;

/// How the Notes tab shares its height among the note bodies it shows (`needs`: each body's content rows, 1 for an
/// empty one; `focus`: the body being edited; `rest_to`: the body that takes the rows nobody needs).
///
/// When every body fits, each gets its need and `rest_to` the rest. Otherwise each gets at least MIN_SECTION_ROWS (or
/// its need), and the rest is split in proportion to what is still missing, the focused body's share counting double.
/// In a pane too small for that, the focused body (else `rest_to`) keeps at least half, and the others get a row each
/// (up to their minimum) while rows are left.
pub fn budget_bodies(total: usize, needs: &[usize], focus: Option<usize>, rest_to: usize) -> Vec<usize> {
    let count = needs.len();
    let rest_to = rest_to.min(count.saturating_sub(1));
    if count <= 1 {
        return needs.iter().map(|_| total).collect();
    }
    let needs: Vec<usize> = needs.iter().map(|need| (*need).max(1)).collect();
    let sum: usize = needs.iter().sum();
    if sum <= total {
        let mut out = needs.clone();
        if let Some(rest) = out.get_mut(rest_to) {
            *rest += total - sum;
        }
        return out;
    }
    let mins: Vec<usize> = needs.iter().map(|need| (*need).min(MIN_SECTION_ROWS)).collect();
    if mins.iter().sum::<usize>() > total {
        // A tiny pane: the focused body (else `rest_to`) first; the others share what is beyond half, a row each first.
        let primary = focus.filter(|at| *at < count).unwrap_or(rest_to);
        let mut share = if total >= 2 { total / 2 } else { 0 };
        let mut out = vec![0; count];
        for round in [1, usize::MAX] {
            for (at, min) in mins.iter().enumerate() {
                if at == primary {
                    continue;
                }
                let want = (*min).min(round).saturating_sub(out[at]).min(share);
                out[at] += want;
                share -= want;
            }
        }
        out[primary] = total - out.iter().sum::<usize>();
        return out;
    }
    let rest = total - mins.iter().sum::<usize>();
    let extras: Vec<usize> = needs.iter().zip(&mins).map(|(need, min)| need - min).collect();
    let weights: Vec<usize> = extras
        .iter()
        .enumerate()
        .map(|(at, extra)| extra * if focus == Some(at) { 2 } else { 1 })
        .collect();
    let total_weight: usize = weights.iter().sum();
    let mut give = vec![0; count];
    let mut left = rest;
    for at in (0..count).filter(|at| *at != rest_to) {
        // Math.round of a non-negative quotient: floor(a / b + 1/2).
        let share = if total_weight > 0 {
            (2 * rest * weights[at] + total_weight) / (2 * total_weight)
        } else {
            0
        };
        give[at] = extras[at].min(share).min(left);
        left -= give[at];
    }
    give[rest_to] = extras[rest_to].min(left);
    left -= give[rest_to];
    // What `rest_to` could not use goes back to the others, in order.
    for at in (0..count).filter(|at| *at != rest_to) {
        let more = (extras[at] - give[at]).min(left);
        give[at] += more;
        left -= more;
    }
    mins.iter().zip(&give).map(|(min, give)| min + give).collect()
}

/// TS: notes.ts checklistLabel (`☐ 2 open` / `☑ 3 done` for a section header; empty without checklist items).
pub fn checklist_label(text: Option<&str>) -> String {
    let ChecklistCounts { open, done } = checklist_counts(text);
    if open > 0 {
        format!("☐ {open} open")
    } else if done > 0 {
        format!("☑ {done} done")
    } else {
        String::new()
    }
}

/// TS: notes.ts fitFirst (the first candidate that fits `width` UTF-16 units, else the last one cut).
pub fn fit_first(candidates: &[String], width: usize) -> String {
    candidates
        .iter()
        .find(|candidate| js_len(candidate) <= width)
        .cloned()
        .unwrap_or_else(|| truncate(candidates.last().map_or("", String::as_str), width))
}

#[cfg(test)]
mod tests {
    use super::*;
    use deckhand_testkit::{Case, replay, tags};
    use serde_json::{Value, json};

    /// `undefined` for `None`, as TS returns it.
    fn maybe<T: Serialize>(value: Option<T>) -> Value {
        value.map_or_else(tags::undefined, |value| {
            serde_json::to_value(value).unwrap_or(Value::Null)
        })
    }

    /// A JS number argument as a Rust count: `Math.floor`, negatives as 0 (every port function clamps them so).
    fn count(case: &Case, index: usize) -> Result<usize, String> {
        let number = case.arg_f64(index)?;
        Ok(if number.is_finite() && number > 0.0 {
            number.floor() as usize
        } else {
            0
        })
    }

    /// An editor state argument; a negative cursor (not a byte offset) is seen as 0, the start of the text.
    fn editor_arg(case: &Case) -> Result<EditorState, String> {
        let mut value = tags::untag(case.arg(0));
        if let Some(cursor) = value.get_mut("cursor")
            && cursor.as_f64().is_some_and(|number| number < 0.0)
        {
            *cursor = json!(0);
        }
        serde_json::from_value(value).map_err(|error| error.to_string())
    }

    fn editor(text: &str, cursor: usize) -> EditorState {
        edited(text.to_owned(), cursor)
    }

    fn rows(list: &[(&str, Option<NoteRowKind>)]) -> Vec<NoteRow> {
        list.iter().map(|(text, kind)| row((*text).to_owned(), *kind)).collect()
    }

    /// The `linkById` lookup the fixtures record.
    fn link_by_id(id: &str) -> TaskLinkState {
        match id.chars().next() {
            Some(first) if first.is_ascii_digit() => TaskLinkState::Done,
            Some('a'..='m') => TaskLinkState::Open,
            _ => TaskLinkState::Gone,
        }
    }

    // ── Parity replays ──────────────────────────────────────────────────────────────────────────────────────────

    #[test]
    fn note_revision_parity() {
        replay("notes/noteRevision", |case: &Case| {
            // Node hashes a lone surrogate as U+FFFD, which is what the lossy untag gives.
            let text = tags::untag(case.arg(0));
            Ok::<_, String>(note_revision(text.as_str().ok_or("text")?))
        });
    }

    #[test]
    fn repo_note_id_parity() {
        replay("notes/repoNoteId", |case: &Case| {
            Ok::<_, String>(repo_note_id(case.arg_str(0)?))
        });
    }

    #[test]
    fn parse_checklist_line_parity() {
        replay("notes/parseChecklistLine", |case: &Case| {
            Ok::<_, String>(maybe(parse_checklist_line(case.arg_str(0)?)))
        });
    }

    #[test]
    fn checklist_counts_parity() {
        replay("notes/checklistCounts", |case: &Case| {
            let text = if case.arg_is_undefined(0) {
                None
            } else {
                Some(case.arg_str(0)?)
            };
            Ok::<_, String>(checklist_counts(text))
        });
    }

    #[test]
    fn toggle_checklist_line_parity() {
        replay("notes/toggleChecklistLine", |case: &Case| {
            // `lines[line]` is undefined for a negative or fractional index.
            let line = case.arg_f64(1)?;
            let result = if line >= 0.0 && line.fract() == 0.0 {
                toggle_checklist_line(case.arg_str(0)?, line as usize)
            } else {
                None
            };
            Ok::<_, String>(maybe(result))
        });
    }

    #[test]
    fn task_link_suffix_parity() {
        replay("notes/taskLinkSuffix", |case: &Case| {
            let state: Option<TaskLinkState> = if case.arg_is_undefined(0) {
                None
            } else {
                Some(case.arg_as(0)?)
            };
            Ok::<_, String>(task_link_suffix(state))
        });
    }

    #[test]
    fn linked_task_id_parity() {
        replay("notes/linkedTaskId", |case: &Case| {
            Ok::<_, String>(maybe(linked_task_id(case.arg_str(0)?)))
        });
    }

    #[test]
    fn toggle_checklist_parity() {
        replay("notes/toggleChecklist", |case: &Case| {
            Ok::<_, String>(toggle_checklist(&editor_arg(case)?))
        });
    }

    #[test]
    fn insert_checklist_item_parity() {
        replay("notes/insertChecklistItem", |case: &Case| {
            Ok::<_, String>(insert_checklist_item(&editor_arg(case)?))
        });
    }

    #[test]
    fn continue_checklist_parity() {
        replay("notes/continueChecklist", |case: &Case| {
            Ok::<_, String>(maybe(continue_checklist(&editor_arg(case)?)))
        });
    }

    #[test]
    fn visible_text_parity() {
        replay("notes/visibleText", |case: &Case| {
            Ok::<_, String>(visible_text(case.arg_str(0)?))
        });
    }

    #[test]
    fn note_read_rows_parity() {
        replay("notes/noteReadRows", |case: &Case| {
            let open = |_: &str| TaskLinkState::Open;
            let done = |_: &str| TaskLinkState::Done;
            let gone = |_: &str| TaskLinkState::Gone;
            let links: Option<TaskLinkLookup<'_>> = match case.fn_name(2) {
                Some("linkOpen") => Some(&open),
                Some("linkDone") => Some(&done),
                Some("linkGone") => Some(&gone),
                Some("linkById") => Some(&link_by_id),
                Some(other) => return Err(format!("unknown lookup {other}")),
                None => None,
            };
            Ok::<_, String>(note_read_rows(case.arg_str(0)?, count(case, 1)?, links))
        });
    }

    #[test]
    fn fit_read_rows_parity() {
        replay("notes/fitReadRows", |case: &Case| {
            Ok::<_, String>(fit_read_rows(case.arg_as(0)?, count(case, 1)?))
        });
    }

    /// The cases recorded for the two sections the tab had (the worktree's note, `shared`, above the session's)
    /// still hold for two bodies, the second taking the rest.
    #[test]
    fn budget_bodies_parity() {
        replay("notes/budgetSections", |case: &Case| {
            let total = count(case, 0)?;
            let session = count(case, 2)?;
            let focus = match case.arg(3).as_str() {
                Some("shared") => Some(0),
                Some("session") => Some(1),
                _ => None,
            };
            let (shared, session) = if case.arg_is_undefined(1) {
                (0, budget_bodies(total, &[session], None, 0)[0])
            } else {
                let bodies = budget_bodies(total, &[count(case, 1)?, session], focus, 1);
                (bodies[0], bodies[1])
            };
            Ok::<_, String>(json!({"shared": shared, "session": session}))
        });
    }

    #[test]
    fn checklist_label_parity() {
        replay("notes/checklistLabel", |case: &Case| {
            let text = if case.arg_is_undefined(0) {
                None
            } else {
                Some(case.arg_str(0)?)
            };
            Ok::<_, String>(checklist_label(text))
        });
    }

    #[test]
    fn fit_first_parity() {
        replay("notes/fitFirst", |case: &Case| {
            // Rust strings cannot hold lone surrogates: the lossy U+FFFD keeps the UTF-16 length, and a candidate
            // returned whole is answered with its original (tagged) value.
            let candidates: Vec<String> = case.arg_as(0)?;
            let result = fit_first(&candidates, count(case, 1)?);
            let original = candidates
                .iter()
                .position(|candidate| *candidate == result && candidate.contains('\u{FFFD}'))
                .and_then(|index| case.arg(0).get(index).cloned());
            Ok::<_, String>(original.unwrap_or(Value::String(result)))
        });
    }

    // ── Ported TS tests ─────────────────────────────────────────────────────────────────────────────────────────

    #[test]
    fn checklists_items_and_counts() {
        assert_eq!(
            parse_checklist_line("- [ ] write tests"),
            Some(ChecklistItem {
                indent: String::new(),
                bullet: "-".into(),
                checked: false,
                prefix: 6,
                text: "write tests".into(),
                box_at: 3,
            })
        );
        assert_eq!(
            parse_checklist_line("  * [x] done").map(|item| item.checked),
            Some(true)
        );
        assert_eq!(
            parse_checklist_line("\t+ [X] Done").map(|item| item.indent).as_deref(),
            Some("\t")
        );
        assert_eq!(parse_checklist_line("- [ ]").map(|item| item.text).as_deref(), Some(""));
        for line in ["- [] no", "-[ ] no", "- [y] no", "text - [ ] no", "- [ ]x"] {
            assert_eq!(parse_checklist_line(line), None, "{line}");
        }
        assert_eq!(
            checklist_counts(Some("- [ ] a\n  - [x] b\n* [X] c\n+ [ ] d\nplain")),
            ChecklistCounts { open: 2, done: 2 }
        );
        assert_eq!(checklist_counts(None), ChecklistCounts { open: 0, done: 0 });
        assert_eq!(checklist_label(Some("- [ ] a\n- [x] b")), "☐ 1 open");
        assert_eq!(checklist_label(Some("- [x] b")), "☑ 1 done");
        assert_eq!(checklist_label(Some("nothing")), "");
    }

    #[test]
    fn ctrl_x_toggles_and_ctrl_t_enter_add_items() {
        assert_eq!(
            toggle_checklist(&editor("a\n  - [ ] b\nc", 5)),
            editor("a\n  - [x] b\nc", 5)
        );
        assert_eq!(toggle_checklist(&editor("* [X] b", 7)), editor("* [ ] b", 7));
        // A plain line gets `- [ ] ` after its indent, a bullet keeps its bullet; the cursor stays on the same text.
        assert_eq!(toggle_checklist(&editor("  fix it", 4)), editor("  - [ ] fix it", 10));
        assert_eq!(toggle_checklist(&editor("- fix it", 0)), editor("- [ ] fix it", 0));
        assert_eq!(toggle_checklist(&editor("", 0)), editor("- [ ] ", 6));
        // Ctrl+T: below the line with its indent (and bullet); an empty line becomes the item.
        assert_eq!(
            insert_checklist_item(&editor("  * [x] a\nb", 3)),
            editor("  * [x] a\n  * [ ] \nb", 18)
        );
        assert_eq!(insert_checklist_item(&editor("title", 0)), editor("title\n- [ ] ", 12));
        assert_eq!(insert_checklist_item(&editor("a\n\nb", 2)), editor("a\n- [ ] \nb", 8));
        // Enter continues a list (splitting the item), ends it on an empty item, and is a plain newline elsewhere.
        assert_eq!(
            continue_checklist(&editor("- [ ] buy milk", 10)),
            Some(editor("- [ ] buy \n- [ ] milk", 17))
        );
        assert_eq!(continue_checklist(&editor("x\n  - [ ] ", 10)), Some(editor("x\n", 2)));
        assert_eq!(continue_checklist(&editor("- [ ] item", 2)), None);
        assert_eq!(continue_checklist(&editor("plain", 5)), None);
    }

    #[test]
    fn read_mode_rows_and_more_lines() {
        use NoteRowKind::{Done, Heading, More, Open};
        let note = note_read_rows(
            "# Plan\n- [ ] write the migration code\n  - [x] done\nplain words wrap here",
            16,
            None,
        );
        assert_eq!(
            note,
            rows(&[
                ("# Plan", Some(Heading)),
                ("☐ write the ", Some(Open)),
                ("  migration code", Some(Open)),
                ("  ☑ done", Some(Done)),
                ("plain words ", None),
                ("wrap here", None),
            ])
        );
        // Tabs and controls are shown safely; a word longer than the width is cut.
        let texts: Vec<String> = note_read_rows("a\tb\x07\nabcdefghij", 4, None)
            .into_iter()
            .map(|row| row.text)
            .collect();
        assert_eq!(texts, ["a b?", "abcd", "efgh", "ij"]);
        assert_eq!(fit_read_rows(note.clone(), 6), note);
        assert_eq!(
            fit_read_rows(note.clone(), 3),
            vec![
                note[0].clone(),
                note[1].clone(),
                row("+4 more lines".into(), Some(More))
            ]
        );
        assert_eq!(
            fit_read_rows(note.clone(), 1),
            vec![row("+6 more lines".into(), Some(More))]
        );
        assert_eq!(
            fit_read_rows(note[..3].to_vec(), 2),
            vec![note[0].clone(), row("+2 more lines".into(), Some(More))]
        );
        assert_eq!(fit_read_rows(note, 0), Vec::<NoteRow>::new());
    }

    #[test]
    fn bodies_share_the_height() {
        let (first, second) = (Some(0), Some(1));
        // Everything fits: each its need, the last body the rest.
        assert_eq!(budget_bodies(20, &[4, 6], None, 1), [4, 16]);
        // An empty (collapsed) body takes one row.
        assert_eq!(budget_bodies(20, &[0, 30], None, 1), [1, 19]);
        // Both too long: 3 each, the rest split by what is missing (equal needs, equal split), focus counts double.
        assert_eq!(budget_bodies(20, &[30, 30], None, 1), [10, 10]);
        assert_eq!(budget_bodies(20, &[30, 30], first, 1), [12, 8]);
        assert_eq!(budget_bodies(20, &[30, 30], second, 1), [8, 12]);
        assert_eq!(budget_bodies(20, &[50, 10], None, 1), [15, 5]);
        // The minimum holds for a short body; a body never gets more than it needs while another wants rows.
        assert_eq!(budget_bodies(10, &[2, 40], first, 1), [2, 8]);
        // Tiny panes: the focused (else the last) body first, a row for each other while there are rows.
        assert_eq!(budget_bodies(4, &[10, 10], None, 1), [2, 2]);
        assert_eq!(budget_bodies(3, &[10, 10], first, 1), [2, 1]);
        assert_eq!(budget_bodies(1, &[10, 10], None, 1), [0, 1]);
        assert_eq!(budget_bodies(5, &[10, 10, 10], Some(2), 1), [1, 1, 3]);
        assert_eq!(budget_bodies(2, &[10, 10, 10], None, 1), [1, 1, 0]);
        // One body: everything.
        assert_eq!(budget_bodies(12, &[40], None, 0), [12]);
        assert!(budget_bodies(12, &[], None, 0).is_empty());
        // Three bodies, the middle one taking the rest: what it cannot use goes to the others in order.
        assert_eq!(budget_bodies(30, &[2, 3, 4], None, 1), [2, 24, 4]);
        assert_eq!(budget_bodies(20, &[30, 4, 30], Some(0), 1), [10, 3, 7]);
        for total in 0..40 {
            for focus in [None, Some(0), Some(1), Some(2)] {
                let bodies = budget_bodies(total, &[7, 25, 3], focus, 1);
                assert_eq!(bodies.iter().sum::<usize>(), total, "{total} {focus:?} {bodies:?}");
            }
        }
    }

    /// noteSteps.test.ts: toggleChecklistLine on a step, and a `↗` line's read row (the rest is P6's).
    #[test]
    fn note_steps_toggle_and_link_rows() {
        let note = "# Plan\n- [x] Read the spec\n- [ ] Write the parser\n  - [ ] Handle tabs\n- ↗ Docs <!-- dh:t=d0c5 -->\n- [ ] \nprose";
        let line = |index: usize| {
            toggle_checklist_line(note, index).and_then(|text| text.split('\n').nth(index).map(str::to_owned))
        };
        assert_eq!(line(2).as_deref(), Some("- [x] Write the parser"));
        assert_eq!(line(1).as_deref(), Some("- [ ] Read the spec"));
        assert_eq!(line(3).as_deref(), Some("  - [x] Handle tabs"));
        assert_eq!(toggle_checklist_line(note, 0), None);
        let links = |id: &str| {
            if id == "bbbb" {
                TaskLinkState::Done
            } else {
                TaskLinkState::Gone
            }
        };
        assert_eq!(
            note_read_rows("- ↗ Done <!-- dh:t=bbbb -->", 40, Some(&links)),
            rows(&[("↗ Done · done", Some(NoteRowKind::Link))])
        );
        assert_eq!(
            note_read_rows("- ↗ Gone <!-- dh:t=cccc -->", 40, None),
            rows(&[("↗ Gone · in Tasks", Some(NoteRowKind::Link))])
        );
        assert_eq!(linked_task_id("- ↗ Docs <!-- dh:t=d0c5 -->"), Some("d0c5"));
    }
}
