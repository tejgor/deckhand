//! Port of notes.ts.
//!
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
use crate::jsstr::{js_len, js_trim, js_trim_end, path};
use crate::text_editor::{EditorState, line_end, line_start, wrap_rows};
use crate::workspace::workspace_key;
use deckhand_protocol::data::{NoteSection, SessionRecord, SharedNoteKind};

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

/// TS: notes.ts sharedNoteIdentity result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedNoteIdentity {
    pub kind: SharedNoteKind,
    pub id: String,
}

/// TS: notes.ts sharedNoteIdentity (the worktree incarnation's note, else the main checkout's; none while preparing).
///
/// The linked worktree's note stays its identity also for a deleted worktree, while the record exists.
pub fn shared_note_identity(session: &SessionRecord) -> Option<SharedNoteIdentity> {
    if let Some(id) = worktree_id(session) {
        return Some(SharedNoteIdentity {
            kind: SharedNoteKind::Worktree,
            id: id.to_owned(),
        });
    }
    workspace_key(session).map(|key| SharedNoteIdentity {
        kind: SharedNoteKind::Repo,
        id: repo_note_id(&key),
    })
}

/// `session.worktree?.id` when truthy.
fn worktree_id(session: &SessionRecord) -> Option<&str> {
    session
        .worktree
        .as_ref()
        .and_then(|worktree| worktree.id.as_deref())
        .filter(|id| !id.is_empty())
}

/// TS: notes.ts showsOwnNote.
///
/// A session in a worktree has one note, the worktree's (it outlives any one session there); a main-checkout session
/// keeps its own (each is a separate job). A leftover note of a worktree session (not merged yet) is never hidden.
pub fn shows_own_note(session: &SessionRecord) -> bool {
    worktree_id(session).is_none()
        || session.notes.as_deref().is_some_and(|notes| !js_trim(notes).is_empty())
        || session
            .notes_file
            .as_ref()
            .is_some_and(|file| file.too_large == Some(true))
}

/// One session note to merge into its worktree's (TS: `{title, text}`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TitledNote {
    pub title: String,
    pub text: String,
}

/// TS: notes.ts mergeIntoWorktreeNote: a lone note into an empty worktree note as it is, else each under a
/// `## <session title>` heading after what the worktree note already says.
pub fn merge_into_worktree_note(worktree_text: &str, notes: &[TitledNote]) -> String {
    let parts: Vec<(&str, &str)> = notes
        .iter()
        .map(|note| (note.title.as_str(), js_trim_end(&note.text)))
        .filter(|(_, text)| !js_trim(text).is_empty())
        .collect();
    if parts.is_empty() {
        return worktree_text.to_owned();
    }
    if let [(_, text)] = parts.as_slice()
        && js_trim(worktree_text).is_empty()
    {
        return (*text).to_owned();
    }
    std::iter::once(js_trim_end(worktree_text).to_owned())
        .chain(parts.iter().map(|(title, text)| format!("## {title}\n{text}")))
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// TS: notes.ts noteKey (the UI's draft key: `session:<id>`, or `<kind>:<id>` of the shared note, the same draft for
/// every session showing it).
pub fn note_key(session: &SessionRecord, section: NoteSection) -> Option<String> {
    match section {
        NoteSection::Session => Some(format!("session:{}", session.id)),
        NoteSection::Shared => session
            .shared_notes
            .as_ref()
            .map(|shared| format!("{}:{}", shared.kind, shared.id)),
    }
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

/// TS: notes.ts budgetSections result.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SectionBudget {
    pub shared: usize,
    pub session: usize,
}

/// TS: notes.ts budgetSections (`shared`: the shared section's need, `None` without one).
///
/// Each section's need is its content rows (1 for an empty, collapsed one). When both fit, each gets its need and the
/// session section the rest; otherwise each gets at least MIN_SECTION_ROWS (or its need), and the rest is split in
/// proportion to what is still missing, the focused section's share counting double. Without a shared section, the
/// session section gets everything.
pub fn budget_sections(
    total: usize,
    shared: Option<usize>,
    session: usize,
    focus: Option<NoteSection>,
) -> SectionBudget {
    let Some(shared) = shared else {
        return SectionBudget {
            shared: 0,
            session: total,
        };
    };
    // Signed arithmetic as in TS; i128 holds every usize sum and product here.
    let wide = |value: usize| i128::try_from(value).unwrap_or(i128::MAX);
    let budget = |shared: i128, session: i128| SectionBudget {
        shared: usize::try_from(shared.max(0)).unwrap_or(usize::MAX),
        session: usize::try_from(session.max(0)).unwrap_or(usize::MAX),
    };
    let rows = wide(total);
    let (need_shared, need_session) = (wide(shared).max(1), wide(session).max(1));
    if need_shared + need_session <= rows {
        return budget(need_shared, rows - need_shared);
    }
    let min_rows = wide(MIN_SECTION_ROWS);
    let (min_shared, min_session) = (need_shared.min(min_rows), need_session.min(min_rows));
    let shared_focused = focus == Some(NoteSection::Shared);
    if min_shared + min_session > rows {
        // A tiny pane: the focused section (else the session's) first, one row for the other while there are two.
        let other_min = if shared_focused { min_session } else { min_shared };
        let other = if rows >= 2 {
            other_min.min(rows - (rows + 1) / 2).max(1)
        } else {
            0
        };
        return if shared_focused {
            budget(rows - other, other)
        } else {
            budget(other, rows - other)
        };
    }
    let rest = rows - min_shared - min_session;
    let (extra_shared, extra_session) = (need_shared - min_shared, need_session - min_session);
    let weight_shared = extra_shared * if shared_focused { 2 } else { 1 };
    let weight_session = extra_session * if focus == Some(NoteSection::Session) { 2 } else { 1 };
    let weights = weight_shared + weight_session;
    // Math.round of a non-negative quotient: floor(a / b + 1/2).
    let mut give_shared = if weights > 0 {
        extra_shared.min((2 * rest * weight_shared + weights) / (2 * weights))
    } else {
        0
    };
    let give_session = extra_session.min(rest - give_shared);
    give_shared = extra_shared.min(rest - give_session);
    budget(min_shared + give_shared, min_session + give_session)
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

/// TS: notes.ts openChecklistText: the sidebar details' checklist line for the selected session, open items of its
/// note plus its worktree's (`☐ 3 open (2 worktree)`), shortened to fit; `None` without open items.
pub fn open_checklist_text(session: &SessionRecord, width: usize) -> Option<String> {
    let own = checklist_counts(session.notes.as_deref()).open;
    let shared = checklist_counts(session.shared_notes.as_ref().map(|shared| shared.text.as_str())).open;
    let total = own + shared;
    if total == 0 || width < 1 {
        return None;
    }
    let mut candidates = Vec::with_capacity(4);
    if shared > 0 {
        candidates.push(format!("☐ {total} open ({shared} worktree)"));
        candidates.push(format!("☐ {total} open ({shared} wt)"));
    }
    candidates.push(format!("☐ {total} open"));
    candidates.push(format!("☐ {total}"));
    Some(fit_first(&candidates, width))
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

    /// A SessionRecord from a partial TS literal (`{...} as SessionRecord`): required fields the code under test never
    /// reads get placeholder values. Fixture literals also use worktree modes outside the TS union (`'worktree'`),
    /// which TS treats as any mode but `'none'`.
    fn partial_session(value: &Value) -> Result<SessionRecord, String> {
        let mut full = json!({
            "id": "", "title": "", "program": "claude", "command": "", "cwd": "", "repoRoot": "",
            "status": "running", "createdAt": "", "updatedAt": "",
        });
        let given = tags::untag(value);
        let (Some(object), Some(given)) = (full.as_object_mut(), given.as_object()) else {
            return Err("not an object".into());
        };
        object.extend(given.clone());
        if let Some(worktree) = object.get_mut("worktree").and_then(Value::as_object_mut)
            && worktree.get("mode") == Some(&json!("worktree"))
        {
            worktree.insert("mode".into(), json!("managed"));
        }
        if object.get("requestedWorktreeMode") == Some(&json!("worktree")) {
            object.insert("requestedWorktreeMode".into(), json!("new"));
        }
        let placeholders: [(&str, &[&str]); 2] = [
            ("notesFile", &["path", "revision"]),
            ("sharedNotes", &["path", "text", "revision"]),
        ];
        for (key, fields) in placeholders {
            if let Some(record) = object.get_mut(key).and_then(Value::as_object_mut) {
                for field in fields {
                    record.entry(*field).or_insert(json!(""));
                }
            }
        }
        serde_json::from_value(full).map_err(|error| error.to_string())
    }

    fn session(value: Value) -> SessionRecord {
        partial_session(&value).expect("a session literal")
    }

    fn section(value: &Value) -> Option<NoteSection> {
        match value.as_str() {
            Some("shared") => Some(NoteSection::Shared),
            Some("session") => Some(NoteSection::Session),
            _ => None,
        }
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
    fn shared_note_identity_parity() {
        replay("notes/sharedNoteIdentity", |case: &Case| {
            Ok::<_, String>(maybe(shared_note_identity(&partial_session(case.arg(0))?)))
        });
    }

    #[test]
    fn shows_own_note_parity() {
        replay("notes/showsOwnNote", |case: &Case| {
            Ok::<_, String>(shows_own_note(&partial_session(case.arg(0))?))
        });
    }

    #[test]
    fn merge_into_worktree_note_parity() {
        replay("notes/mergeIntoWorktreeNote", |case: &Case| {
            let notes: Vec<TitledNote> = case.arg_as(1)?;
            Ok::<_, String>(merge_into_worktree_note(case.arg_str(0)?, &notes))
        });
    }

    #[test]
    fn note_key_parity() {
        replay("notes/noteKey", |case: &Case| {
            let section = section(case.arg(1)).ok_or("section")?;
            Ok::<_, String>(maybe(note_key(&partial_session(case.arg(0))?, section)))
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

    #[test]
    fn budget_sections_parity() {
        replay("notes/budgetSections", |case: &Case| {
            let shared = if case.arg_is_undefined(1) {
                None
            } else {
                Some(count(case, 1)?)
            };
            Ok::<_, String>(budget_sections(
                count(case, 0)?,
                shared,
                count(case, 2)?,
                section(case.arg(3)),
            ))
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

    #[test]
    fn open_checklist_text_parity() {
        replay("notes/openChecklistText", |case: &Case| {
            Ok::<_, String>(maybe(open_checklist_text(
                &partial_session(case.arg(0))?,
                count(case, 1)?,
            )))
        });
    }

    // ── Ported TS tests ─────────────────────────────────────────────────────────────────────────────────────────

    // covers: B-P3-01 checklists: -, * and + items, indented, [x] and [X]; counts
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

    // covers: B-P3-02 ctrl+x toggles the cursor line's item or makes the line one; ctrl+t and enter add items
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

    // covers: B-P3-03 read mode: wrapped rows, ☐/☑ items with a hanging indent, headings, and +N more lines when cut
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

    // covers: B-P3-04 the two sections share the height: needs when they fit, else a minimum each and a proportional split favouring focus
    #[test]
    fn sections_share_the_height() {
        let budget = |shared, session| SectionBudget { shared, session };
        let (shared, session) = (Some(NoteSection::Shared), Some(NoteSection::Session));
        // Everything fits: each its need, the session section the rest.
        assert_eq!(budget_sections(20, Some(4), 6, None), budget(4, 16));
        // An empty (collapsed) section takes one row.
        assert_eq!(budget_sections(20, Some(1), 30, None), budget(1, 19));
        // Both too long: 3 each, the rest split by what is missing (equal needs, equal split), focus counts double.
        assert_eq!(budget_sections(20, Some(30), 30, None), budget(10, 10));
        assert_eq!(budget_sections(20, Some(30), 30, shared), budget(12, 8));
        assert_eq!(budget_sections(20, Some(30), 30, session), budget(8, 12));
        assert_eq!(budget_sections(20, Some(50), 10, None), budget(15, 5));
        // The minimum holds for a short section; a section never gets more than it needs while the other wants rows.
        assert_eq!(budget_sections(10, Some(2), 40, shared), budget(2, 8));
        // Tiny panes: the focused (else the session's) section first, one row for the other.
        assert_eq!(budget_sections(4, Some(10), 10, None), budget(2, 2));
        assert_eq!(budget_sections(3, Some(10), 10, shared), budget(2, 1));
        assert_eq!(budget_sections(1, Some(10), 10, None), budget(0, 1));
        // No shared section: the session's gets everything.
        assert_eq!(budget_sections(12, None, 40, None), budget(0, 12));
    }

    // covers: B-P3-05 one note per worktree: a session there shows only the worktree's (unless it still has its own); merging keeps every word
    #[test]
    fn one_note_per_worktree() {
        let at = json!({"worktree": {"mode": "managed", "id": "w1", "path": "/wt"}});
        assert!(shows_own_note(&session(json!({}))), "main checkout");
        assert!(!shows_own_note(&session(at.clone())));
        let mut leftover = at;
        leftover["notes"] = json!("left over");
        assert!(shows_own_note(&session(leftover)), "a leftover note is never hidden");
        let note = |title: &str, text: &str| TitledNote {
            title: title.into(),
            text: text.into(),
        };
        assert_eq!(merge_into_worktree_note("", &[note("only", "- [ ] a\n")]), "- [ ] a");
        assert_eq!(
            merge_into_worktree_note("plan\n", &[note("one", "x"), note("two", "  "), note("three", "y\n")]),
            "plan\n\n## one\nx\n\n## three\ny"
        );
        assert_eq!(
            merge_into_worktree_note("", &[note("one", "x"), note("two", "y")]),
            "## one\nx\n\n## two\ny"
        );
        assert_eq!(merge_into_worktree_note("kept", &[]), "kept");
    }

    // covers: B-P3-06 shared note identity: the worktree record, else the main checkout's repository note; none while preparing
    #[test]
    fn shared_note_identity_worktree_else_repo() {
        let identity = |kind, id: &str| {
            Some(SharedNoteIdentity {
                kind,
                id: id.to_owned(),
            })
        };
        let repo = repo_note_id("/repo");
        let of = |value: Value| shared_note_identity(&session(value));
        assert_eq!(
            of(json!({"cwd": "/wt/a", "worktree": {"mode": "none", "id": "rec"}})),
            identity(SharedNoteKind::Worktree, "rec")
        );
        assert_eq!(
            of(
                json!({"cwd": "/wt/a", "worktree": {"mode": "managed", "id": "rec", "path": "/wt/a", "deletedAt": "yesterday"}})
            ),
            identity(SharedNoteKind::Worktree, "rec")
        );
        assert_eq!(
            of(json!({"cwd": "/repo/sub", "launchWorktreeRoot": "/repo", "worktree": {"mode": "none"}})),
            identity(SharedNoteKind::Repo, &repo)
        );
        assert_eq!(
            of(json!({"cwd": "/repo", "worktree": {"mode": "attached", "path": "/repo/", "isMain": true}})),
            identity(SharedNoteKind::Repo, &repo)
        );
        assert_eq!(of(json!({"cwd": "/repo", "requestedWorktreeMode": "new"})), None);
        assert!(repo.len() == 16 && repo.chars().all(|character| matches!(character, '0'..='9' | 'a'..='f')));
    }

    // The P3 parts of TS tests deferred to the units owning the rest of them.

    /// notes.test.ts "sidebar details: ..." (openChecklistText; sessionDetails is P8's).
    #[test]
    fn open_checklist_text_shortened_to_fit() {
        let with = |fields: Value| {
            let mut value = json!({"id": "s", "title": "task", "program": "claude", "command": "claude",
                "cwd": "/repo", "repoRoot": "/repo", "status": "running", "agentStatus": "idle",
                "createdAt": "1970-01-01T00:00:00.000Z", "updatedAt": "1970-01-01T00:00:00.000Z"});
            if let (Some(object), Some(fields)) = (value.as_object_mut(), fields.as_object()) {
                object.extend(fields.clone());
            }
            session(value)
        };
        let with_notes = with(
            json!({"notes": "- [ ] a", "sharedNotes": {"kind": "worktree", "id": "w",
            "path": "/n/w.md", "revision": "r", "text": "- [ ] b\n- [ ] c\n- [x] d"}}),
        );
        assert_eq!(
            open_checklist_text(&with_notes, 40).as_deref(),
            Some("☐ 3 open (2 worktree)")
        );
        assert_eq!(open_checklist_text(&with_notes, 16).as_deref(), Some("☐ 3 open (2 wt)"));
        assert_eq!(open_checklist_text(&with_notes, 9).as_deref(), Some("☐ 3 open"));
        assert_eq!(open_checklist_text(&with_notes, 4).as_deref(), Some("☐ 3"));
        assert_eq!(
            open_checklist_text(&with(json!({"notes": "- [ ] a"})), 40).as_deref(),
            Some("☐ 1 open")
        );
        assert_eq!(open_checklist_text(&with(json!({"notes": "- [x] a"})), 40), None);
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
