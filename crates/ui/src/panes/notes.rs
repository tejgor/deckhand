//! The Notes tab's rows, top to bottom: the session's line and its steps, the line's brief (folded unless opened), the
//! line's note, the session's own note, and for a sub-session its parent's line note (folded unless opened), the notes
//! each under a header. Read mode shows rendered checklists from the top (`+N more lines` when cut); the note being
//! edited shows the raw Markdown with a cursor, scrolled to keep it visible. Layout is pure ([`notes_layout`]) so it is
//! unit-tested.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;

use deckhand_core::display::truncate;
use deckhand_core::jsstr::{js_len, js_trim, js_trim_end};
use deckhand_core::notes::{
    NoteRowKind, budget_bodies, checklist_label, fit_first, fit_read_rows, note_read_rows, visible_text,
};
use deckhand_core::text_editor::{EditorState, scroll_top_for, wrapped_editor_lines};

use crate::primitives::{Row, Seg, TextStyle, Wrap, draw_rows, fit_row, truncate_row};
use crate::theme::THEME;

/// A note being edited (TS: notesPane.tsx NotesSectionInput `editing`): the cursor (a byte offset) and the first row shown before this
/// render.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NotesEditing {
    pub cursor: usize,
    pub scroll_top: usize,
}

/// TS: notesPane.tsx NotesSectionInput.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NotesSectionInput {
    /// Header, longest first; the first that fits (with the checklist count) is shown.
    pub titles: Vec<String>,
    pub text: String,
    /// The one muted line of an empty section that is not being edited.
    pub empty: String,
    /// Shown after the title (warning color): read-only, too large.
    pub flag: Option<String>,
    pub editing: Option<NotesEditing>,
    /// The note E opens and Enter edits (header in bold).
    pub active: bool,
}

/// TS: notesPane.tsx NotesSpan.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NotesSpan {
    pub text: String,
    pub color: Option<Color>,
    pub bold: bool,
    pub dim: bool,
    pub inverse: bool,
}

/// TS: notesPane.tsx NotesRow.
pub type NotesRow = Vec<NotesSpan>;

/// A part with only its text.
fn plain(text: impl Into<String>) -> NotesSpan {
    NotesSpan {
        text: text.into(),
        ..NotesSpan::default()
    }
}

/// A part with a color.
fn colored(text: impl Into<String>, color: Color) -> NotesSpan {
    NotesSpan {
        text: text.into(),
        color: Some(color),
        ..NotesSpan::default()
    }
}

/// The editor state of a section being edited, its cursor kept inside the text and on a character boundary (the
/// editor never leaves it elsewhere; this only keeps a stray offset from reaching a slice).
fn editor_state(section: &NotesSectionInput, editing: &NotesEditing) -> EditorState {
    let mut cursor = editing.cursor.min(section.text.len());
    while !section.text.is_char_boundary(cursor) {
        cursor -= 1;
    }
    EditorState {
        text: section.text.clone(),
        cursor,
        ..EditorState::default()
    }
}

/// TS: notesPane.tsx header: the section's header row (title, flag, checklist count).
pub fn header(section: &NotesSectionInput, width: usize, focused: bool) -> NotesRow {
    let count = checklist_label(Some(&section.text));
    let flag = section.flag.as_deref().filter(|flag| !flag.is_empty());
    let count = Some(count.as_str()).filter(|count| !count.is_empty());
    let tail = [flag, count].into_iter().flatten().collect::<Vec<_>>().join(" · ");
    let tail_len = js_len(&tail);
    let title_width = if tail.is_empty() {
        width
    } else {
        width.saturating_sub(tail_len + 3).max(1)
    };
    let title = fit_first(&section.titles, title_width);
    let fits = js_len(&title) + 3 + tail_len <= width;
    let mut parts = vec![NotesSpan {
        text: title,
        color: Some(if focused { THEME.active } else { THEME.accent_soft }),
        bold: focused || section.active,
        ..NotesSpan::default()
    }];
    if !tail.is_empty() && fits {
        if let Some(flag) = flag {
            parts.push(colored(" · ", THEME.muted));
            parts.push(colored(flag, THEME.warn));
        }
        if let Some(count) = count {
            parts.push(colored(" · ", THEME.muted));
            parts.push(colored(count, THEME.muted));
        }
    }
    parts
}

/// TS: notesPane.tsx need: body rows a section needs at `width`: the editor's rows, one for an empty section, else
/// its rendered rows.
pub fn need(section: &NotesSectionInput, width: usize) -> usize {
    if let Some(editing) = &section.editing {
        return wrapped_editor_lines(&editor_state(section, editing), width).lines.len();
    }
    if js_trim(&section.text).is_empty() {
        1
    } else {
        note_read_rows(js_trim_end(&section.text), width, None).len()
    }
}

/// TS: notesPane.tsx body's result: the rows, and the edited section's first row for the next render.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NotesBody {
    pub rows: Vec<NotesRow>,
    pub scroll_top: Option<usize>,
}

/// TS: notesPane.tsx body: a section's body in `rows` rows.
pub fn body(section: &NotesSectionInput, width: usize, rows: usize) -> NotesBody {
    if rows == 0 {
        return NotesBody::default();
    }
    if let Some(editing) = &section.editing {
        let editor = wrapped_editor_lines(&editor_state(section, editing), width);
        let scroll_top = scroll_top_for(editing.scroll_top, editor.cursor_row, rows, editor.lines.len());
        let shown = editor
            .lines
            .iter()
            .skip(scroll_top)
            .take(rows)
            .map(|line| {
                let mut row = vec![plain(visible_text(&line.before))];
                if let Some(cursor) = &line.cursor {
                    row.push(NotesSpan {
                        inverse: true,
                        ..plain(visible_text(cursor))
                    });
                }
                if !line.after.is_empty() {
                    row.push(plain(visible_text(&line.after)));
                }
                row
            })
            .collect();
        return NotesBody {
            rows: shown,
            scroll_top: Some(scroll_top),
        };
    }
    if js_trim(&section.text).is_empty() {
        return NotesBody {
            rows: vec![vec![colored(truncate(&section.empty, width), THEME.muted)]],
            scroll_top: None,
        };
    }
    // Read mode leaves out the blank lines a note ends with.
    let shown = fit_read_rows(note_read_rows(js_trim_end(&section.text), width, None), rows)
        .into_iter()
        .map(|row| {
            vec![match row.kind {
                Some(NoteRowKind::Done) => colored(row.text, THEME.muted),
                Some(NoteRowKind::Link) => colored(row.text, THEME.accent_soft),
                Some(NoteRowKind::More) => NotesSpan {
                    dim: true,
                    ..colored(row.text, THEME.muted)
                },
                Some(NoteRowKind::Heading) => NotesSpan {
                    bold: true,
                    ..plain(row.text)
                },
                Some(NoteRowKind::Open) | None => plain(row.text),
            }]
        })
        .collect();
    NotesBody {
        rows: shown,
        scroll_top: None,
    }
}

/// The parts of the tab that take focus, in `tab` order (the brief only while it is open).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum NotePart {
    /// The session's line and its steps.
    Steps,
    /// The line's brief.
    Brief,
    /// The line's note.
    Line,
    /// The session's own note.
    #[default]
    Session,
}

/// One step: a line under the session's line.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StepRow {
    pub title: String,
    pub done: bool,
}

/// A step's title being written: renaming step `at`, or a new step (`at` none, below the others).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StepEditing {
    pub at: Option<usize>,
    pub title: EditorState,
}

/// The session's line and its steps.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StepsInput {
    /// The title of the line the session is on (none: it is on no line yet), and whether it is ticked.
    pub line: Option<(String, bool)>,
    pub steps: Vec<StepRow>,
    /// The selected step, while the steps have the focus.
    pub selected: Option<usize>,
    pub editing: Option<StepEditing>,
    /// The steps have the focus (their keys work).
    pub focused: bool,
}

/// A note shown folded to one row until opened: the brief, a sub-session's parent's line note.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FoldInput {
    /// The folded row, after `▸ `.
    pub label: String,
    pub open: bool,
    /// The note when open (its titles start with `▾ `).
    pub section: NotesSectionInput,
}

/// What the tab shows (see the module docs).
#[derive(Clone, Copy, Debug)]
pub struct NotesLayoutInput<'a> {
    pub steps: &'a StepsInput,
    pub brief: Option<&'a FoldInput>,
    pub line: &'a NotesSectionInput,
    pub session: &'a NotesSectionInput,
    pub parent: Option<&'a FoldInput>,
    pub width: usize,
    pub height: usize,
    /// The part with the focus (none outside notes focus).
    pub focus: Option<NotePart>,
}

/// The tab's rows and what the edited note got.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NotesLayout {
    /// At most `height` rows (exactly `height` when everything has a row).
    pub rows: Vec<NotesRow>,
    /// The edited note's first row, for the next render.
    pub scroll_top: Option<usize>,
    /// The body rows the edited note got (none when no note is edited).
    pub edited_rows: Option<usize>,
}

/// The step block's rows at most: a third of the pane, at least 3 (the line's row included).
fn steps_cap(height: usize) -> usize {
    (height / 3).max(3)
}

fn rule(columns: usize) -> NotesRow {
    vec![colored("─".repeat(columns), THEME.border)]
}

fn muted_row(text: &str, columns: usize) -> NotesRow {
    vec![colored(truncate(text, columns), THEME.muted)]
}

/// An edited one-line title: the text with the cursor cell inverted.
fn title_editor_spans(state: &EditorState) -> Vec<NotesSpan> {
    let mut cursor = state.cursor.min(state.text.len());
    while !state.text.is_char_boundary(cursor) {
        cursor -= 1;
    }
    let (before, rest) = state.text.split_at(cursor);
    let mut chars = rest.chars();
    let at = chars.next().map_or_else(|| " ".to_owned(), String::from);
    let mut spans = Vec::new();
    if !before.is_empty() {
        spans.push(plain(visible_text(before)));
    }
    spans.push(NotesSpan {
        inverse: true,
        ..plain(visible_text(&at))
    });
    let after: String = chars.collect();
    if !after.is_empty() {
        spans.push(plain(visible_text(&after)));
    }
    spans
}

/// The line's row, then a row per step (`› ` marks the selected one while the steps have the focus), within
/// [`steps_cap`]; the rest of the steps scroll to the selected one, or are counted (`+N more steps`).
pub fn steps_rows(input: &StepsInput, columns: usize, height: usize) -> Vec<NotesRow> {
    let focused = input.focused;
    let head_color = if focused { THEME.active } else { THEME.accent_soft };
    let mut rows: Vec<NotesRow> = Vec::new();
    let Some((title, done)) = &input.line else {
        let text = if focused {
            "No line yet · a adds a step (it gets its line)"
        } else {
            "No line yet"
        };
        rows.push(vec![NotesSpan {
            bold: focused,
            ..colored(
                truncate(text, columns),
                if focused { THEME.active } else { THEME.muted },
            )
        }]);
        if let Some(editing) = &input.editing {
            rows.push(step_editor_row(editing));
        }
        return rows;
    };
    let ticked = input.steps.iter().filter(|step| step.done).count();
    let mut tail = Vec::new();
    if !input.steps.is_empty() {
        tail.push(format!("{ticked}/{} steps", input.steps.len()));
    }
    if *done {
        tail.push("done".to_owned());
    }
    let tail = if tail.is_empty() {
        String::new()
    } else {
        format!(" · {}", tail.join(" · "))
    };
    let title_width = columns.saturating_sub(js_len(&tail) + 2).max(4);
    let mut head = vec![
        colored("◆ ", THEME.accent),
        NotesSpan {
            bold: true,
            ..colored(truncate(title, title_width), head_color)
        },
    ];
    if !tail.is_empty() {
        head.push(colored(tail, THEME.muted));
    }
    rows.push(head);
    let room = steps_cap(height).saturating_sub(1).max(1);
    let editing_new = input.editing.as_ref().filter(|editing| editing.at.is_none());
    let count = input.steps.len() + usize::from(editing_new.is_some());
    if count == 0 {
        let text = if focused {
            "  No steps yet · a adds one"
        } else {
            "  No steps yet"
        };
        rows.push(muted_row(text, columns));
        return rows;
    }
    // The rows shown: all of them when they fit; else a window around the selection (focused), or the first ones and
    // how many more.
    let selected = editing_new
        .map(|_| input.steps.len())
        .or(input.selected)
        .unwrap_or(0)
        .min(count - 1);
    let (start, shown, more) = if count <= room {
        (0, count, 0)
    } else if focused {
        let start = selected.saturating_sub(room - 1).min(count - room);
        (start, room, 0)
    } else {
        (0, room - 1, count - (room - 1))
    };
    for at in start..start + shown {
        let marker = if focused && at == selected { "› " } else { "  " };
        let marker = colored(marker, THEME.active);
        if let Some(editing) = input
            .editing
            .as_ref()
            .filter(|editing| editing.at.unwrap_or(input.steps.len()) == at)
        {
            let mut row = vec![marker, plain("☐ ")];
            row.extend(title_editor_spans(&editing.title));
            rows.push(row);
            continue;
        }
        let Some(step) = input.steps.get(at) else {
            continue;
        };
        let check = if step.done { "☑ " } else { "☐ " };
        let text = format!("{check}{}", visible_text(&step.title));
        rows.push(vec![
            marker,
            NotesSpan {
                bold: focused && at == selected,
                ..if step.done {
                    colored(text, THEME.muted)
                } else {
                    plain(text)
                }
            },
        ]);
    }
    if more > 0 {
        rows.push(muted_row(
            &format!("  +{more} more step{}", if more == 1 { "" } else { "s" }),
            columns,
        ));
    }
    rows
}

/// The editor row of a new step under a session with no line yet.
fn step_editor_row(editing: &StepEditing) -> NotesRow {
    let mut row = vec![colored("› ", THEME.active), plain("☐ ")];
    row.extend(title_editor_spans(&editing.title));
    row
}

/// A folded note's one row (`▸ label`).
fn folded_row(fold: &FoldInput, columns: usize) -> NotesRow {
    vec![colored(
        truncate(&format!("▸ {}", fold.label), columns),
        THEME.accent_soft,
    )]
}

/// The tab's rows: the steps block, the brief, a rule, the line's note, a rule, the session's note, and the parent's
/// note, then blank rows to the pane's height. The note bodies share the rows the rest leaves ([`budget_bodies`]); the
/// edited note's share counts double when there is not enough.
pub fn notes_layout(input: &NotesLayoutInput<'_>) -> NotesLayout {
    let columns = input.width.max(1);
    let height = input.height;
    let steps = steps_rows(input.steps, columns, height);
    let fixed = steps.len() + usize::from(input.brief.is_some()) + 4 + usize::from(input.parent.is_some());
    // The bodies, in the order they are drawn.
    // Each with the part it is (none: the parent's note, which is not edited here).
    let mut bodies: Vec<(Option<NotePart>, &NotesSectionInput)> = Vec::new();
    if let Some(brief) = input.brief.filter(|fold| fold.open) {
        bodies.push((Some(NotePart::Brief), &brief.section));
    }
    bodies.push((Some(NotePart::Line), input.line));
    bodies.push((Some(NotePart::Session), input.session));
    if let Some(parent) = input.parent.filter(|fold| fold.open) {
        bodies.push((None, &parent.section));
    }
    let needs: Vec<usize> = bodies.iter().map(|(_, section)| need(section, columns)).collect();
    let focus = input
        .focus
        .and_then(|part| bodies.iter().position(|(which, _)| *which == Some(part)));
    let rest_to = bodies
        .iter()
        .position(|(which, _)| *which == Some(NotePart::Session))
        .unwrap_or(0);
    let mut budget = budget_bodies(height.saturating_sub(fixed), &needs, focus, rest_to);
    // The rows nobody needs go below everything, so the parent's note follows the session's.
    let mut spare = 0;
    if let (Some(given), Some(need)) = (budget.get_mut(rest_to), needs.get(rest_to))
        && *given > *need
    {
        spare = *given - *need;
        *given = *need;
    }
    let mut rows = steps;
    let mut scroll_top = None;
    let mut edited_rows = None;
    let mut add = |rows: &mut Vec<NotesRow>, part: Option<NotePart>, section: &NotesSectionInput| {
        let at = bodies.iter().position(|(which, _)| *which == part);
        let count = at.and_then(|at| budget.get(at)).copied().unwrap_or(0);
        let focused = part.is_some() && input.focus == part;
        rows.push(header(section, columns, focused));
        let shown = body(section, columns, count);
        if shown.scroll_top.is_some() {
            scroll_top = shown.scroll_top;
            edited_rows = Some(count);
        }
        let shown_rows = shown.rows.len();
        rows.extend(shown.rows);
        for _ in shown_rows..count {
            rows.push(vec![plain(" ")]);
        }
    };
    if let Some(brief) = input.brief {
        if brief.open {
            add(&mut rows, Some(NotePart::Brief), &brief.section);
        } else {
            rows.push(folded_row(brief, columns));
        }
    }
    rows.push(rule(columns));
    add(&mut rows, Some(NotePart::Line), input.line);
    rows.push(rule(columns));
    add(&mut rows, Some(NotePart::Session), input.session);
    if let Some(parent) = input.parent {
        if parent.open {
            add(&mut rows, None, &parent.section);
        } else {
            rows.push(folded_row(parent, columns));
        }
    }
    rows.extend((0..spare).map(|_| vec![plain(" ")]));
    rows.truncate(height.max(1));
    NotesLayout {
        rows,
        scroll_top,
        edited_rows,
    }
}

/// The styled row of a NotesRow (a blank row renders as one space, as Ink drops empty ones).
pub fn notes_row(row: &NotesRow) -> Row {
    if row.iter().all(|part| part.text.is_empty()) {
        return vec![Seg::plain(" ")];
    }
    row.iter()
        .map(|part| {
            Seg::new(
                part.text.clone(),
                TextStyle {
                    color: part.color,
                    bold: part.bold.then_some(true),
                    dim: part.dim.then_some(true),
                    inverse: part.inverse.then_some(true),
                    ..TextStyle::default()
                },
            )
        })
        .collect()
}

/// TS: notesPane.tsx NotesRows.
pub fn render_notes_rows(rows: &[NotesRow], buf: &mut Buffer, area: Rect) {
    let width = usize::from(area.width);
    let lines: Vec<Row> = rows
        .iter()
        .map(|row| truncate_row(&notes_row(row), width, Wrap::TruncateEnd))
        .collect();
    draw_rows(buf, area, &lines);
}

/// TS: notesPane.tsx NotesMessage: one muted line.
pub fn render_notes_message(text: &str, buf: &mut Buffer, area: Rect) {
    let width = usize::from(area.width);
    let row = vec![Seg::new(truncate(text, width), TextStyle::fg(THEME.muted))];
    // A wide character can push the line past the box: Ink wraps it.
    draw_rows(buf, area, &fit_row(&row, width, Wrap::Wrap));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn section(titles: &[&str], text: &str, empty: &str) -> NotesSectionInput {
        NotesSectionInput {
            titles: titles.iter().map(|title| (*title).to_owned()).collect(),
            text: text.to_owned(),
            empty: empty.to_owned(),
            ..NotesSectionInput::default()
        }
    }

    fn steps(line: Option<&str>, list: &[(&str, bool)]) -> StepsInput {
        StepsInput {
            line: line.map(|title| (title.to_owned(), false)),
            steps: list
                .iter()
                .map(|(title, done)| StepRow {
                    title: (*title).to_owned(),
                    done: *done,
                })
                .collect(),
            ..StepsInput::default()
        }
    }

    fn line_note(text: &str) -> NotesSectionInput {
        section(
            &[
                "Token storage · shared with whoever works on it",
                "Token storage · shared",
                "Token storage",
            ],
            text,
            "No note for this line yet",
        )
    }

    fn session_note(text: &str) -> NotesSectionInput {
        section(
            &["This session · only this conversation", "This session"],
            text,
            "No notes for this session",
        )
    }

    fn fold(label: &str, text: &str, open: bool) -> FoldInput {
        FoldInput {
            label: label.to_owned(),
            open,
            section: section(&[&format!("▾ {label}"), "▾"], text, ""),
        }
    }

    struct Tab {
        steps: StepsInput,
        brief: Option<FoldInput>,
        line: NotesSectionInput,
        session: NotesSectionInput,
        parent: Option<FoldInput>,
        focus: Option<NotePart>,
    }

    impl Tab {
        fn new(steps: StepsInput, line: NotesSectionInput, session: NotesSectionInput) -> Tab {
            Tab {
                steps,
                brief: None,
                line,
                session,
                parent: None,
                focus: None,
            }
        }

        fn layout(&self, width: usize, height: usize) -> NotesLayout {
            notes_layout(&NotesLayoutInput {
                steps: &self.steps,
                brief: self.brief.as_ref(),
                line: &self.line,
                session: &self.session,
                parent: self.parent.as_ref(),
                width,
                height,
                focus: self.focus,
            })
        }

        /// The rows drawn through the renderer, trailing spaces trimmed.
        fn render(&self, width: usize, height: usize) -> Vec<String> {
            let layout = self.layout(width, height);
            let area = Rect::new(
                0,
                0,
                u16::try_from(width).unwrap_or(u16::MAX),
                u16::try_from(height).unwrap_or(u16::MAX),
            );
            let mut buf = Buffer::empty(area);
            render_notes_rows(&layout.rows, &mut buf, area);
            (0..area.height)
                .map(|y| {
                    (0..area.width)
                        .map(|x| buf[(x, y)].symbol().to_owned())
                        .collect::<String>()
                        .trim_end()
                        .to_owned()
                })
                .collect()
        }
    }

    fn joined(row: &NotesRow) -> String {
        row.iter().map(|span| span.text.as_str()).collect()
    }

    #[test]
    fn the_line_its_steps_the_folded_brief_then_both_notes() {
        let mut tab = Tab::new(
            steps(
                Some("Token storage"),
                &[("schema", true), ("encryption", false), ("refresh tokens", false)],
            ),
            line_note("chose AES-GCM\n- [ ] rotate keys"),
            session_note("Review crypto.rs before merging"),
        );
        tab.brief = Some(fold(
            "Brief · from ◎ Rework auth · 14 Nov",
            "Store tokens encrypted.",
            false,
        ));
        let rule = "─".repeat(70);
        assert_eq!(
            tab.render(70, 14),
            [
                "◆ Token storage · 1/3 steps",
                "  ☑ schema",
                "  ☐ encryption",
                "  ☐ refresh tokens",
                "▸ Brief · from ◎ Rework auth · 14 Nov",
                &rule,
                "Token storage · shared with whoever works on it · ☐ 1 open",
                "chose AES-GCM",
                "☐ rotate keys",
                &rule,
                "This session · only this conversation",
                "Review crypto.rs before merging",
                "",
                "",
            ]
        );
        // A narrower pane drops the longer titles before the checklist count, then cuts the title.
        assert_eq!(tab.render(40, 14)[6], "Token storage · shared · ☐ 1 open");
        assert_eq!(tab.render(22, 14)[6], "Token stor… · ☐ 1 open");
        // An empty note is one muted row.
        tab.line = line_note("");
        let rows = tab.layout(70, 14);
        assert_eq!(joined(&rows.rows[7]), "No note for this line yet");
        assert_eq!(rows.rows[7][0].color, Some(THEME.muted));
        assert_eq!(rows.rows.len(), 14);
    }

    #[test]
    fn a_session_on_no_line_says_so_and_a_step_can_be_written_below() {
        let mut tab = Tab::new(steps(None, &[]), line_note(""), session_note("mine"));
        assert_eq!(tab.render(50, 8)[..2], ["No line yet", &"─".repeat(50)]);
        tab.steps.focused = true;
        tab.focus = Some(NotePart::Steps);
        assert_eq!(tab.render(50, 8)[0], "No line yet · a adds a step (it gets its line)");
        tab.steps.editing = Some(StepEditing {
            at: None,
            title: EditorState {
                text: "Reproduce".to_owned(),
                cursor: 9,
                ..EditorState::default()
            },
        });
        let layout = tab.layout(50, 8);
        assert_eq!(joined(&layout.rows[1]), "› ☐ Reproduce ");
        assert!(layout.rows[1].last().is_some_and(|span| span.inverse));
    }

    #[test]
    fn a_line_without_steps_and_a_ticked_line() {
        let mut tab = Tab::new(steps(Some("Docs"), &[]), line_note(""), session_note(""));
        assert_eq!(tab.render(40, 8)[..2], ["◆ Docs", "  No steps yet"]);
        tab.steps.focused = true;
        assert_eq!(tab.render(40, 8)[1], "  No steps yet · a adds one");
        tab.steps.line = Some(("Docs".to_owned(), true));
        tab.steps.steps = vec![StepRow {
            title: "write".to_owned(),
            done: true,
        }];
        assert_eq!(tab.render(40, 8)[0], "◆ Docs · 1/1 steps · done");
    }

    #[test]
    fn steps_take_a_third_of_the_pane_scrolling_to_the_selected_one_while_focused() {
        let many: Vec<(String, bool)> = (1..=9).map(|n| (format!("step {n}"), false)).collect();
        let list: Vec<(&str, bool)> = many.iter().map(|(title, done)| (title.as_str(), *done)).collect();
        let mut tab = Tab::new(steps(Some("Big"), &list), line_note(""), session_note(""));
        // 18 rows: 6 for the block, the line's row and 5 more; unfocused, the last says how many are not shown.
        let rows = tab.render(40, 18);
        assert_eq!(
            rows[..6],
            [
                "◆ Big · 0/9 steps",
                "  ☐ step 1",
                "  ☐ step 2",
                "  ☐ step 3",
                "  ☐ step 4",
                "  +5 more steps"
            ]
        );
        tab.steps.focused = true;
        tab.steps.selected = Some(7);
        let rows = tab.render(40, 18);
        assert_eq!(
            rows[..6],
            [
                "◆ Big · 0/9 steps",
                "  ☐ step 4",
                "  ☐ step 5",
                "  ☐ step 6",
                "  ☐ step 7",
                "› ☐ step 8"
            ]
        );
        // Renaming a step edits it in place.
        tab.steps.editing = Some(StepEditing {
            at: Some(7),
            title: EditorState {
                text: "step eight".to_owned(),
                cursor: 0,
                ..EditorState::default()
            },
        });
        let layout = tab.layout(40, 18);
        assert_eq!(joined(&layout.rows[5]), "› ☐ step eight");
        assert!(layout.rows[5][2].inverse);
    }

    #[test]
    fn an_open_brief_is_a_note_and_the_edited_note_scrolls_to_the_cursor() {
        let text: String = (1..=12).map(|n| format!("row {n}")).collect::<Vec<_>>().join("\n");
        let mut tab = Tab::new(steps(Some("Auth"), &[]), line_note("why"), session_note(""));
        tab.brief = Some(fold("Brief · from you", "set up\n- [ ] in scope", true));
        tab.session = NotesSectionInput {
            editing: Some(NotesEditing {
                cursor: text.len(),
                scroll_top: 0,
            }),
            ..session_note(&text)
        };
        tab.focus = Some(NotePart::Session);
        let layout = tab.layout(40, 16);
        let rows: Vec<String> = layout.rows.iter().map(joined).collect();
        assert_eq!(
            rows,
            [
                "◆ Auth",
                "  No steps yet",
                "▾ Brief · from you · ☐ 1 open",
                "set up",
                "☐ in scope",
                &"─".repeat(40),
                "Token storage · shared",
                "why",
                &"─".repeat(40),
                "This session · only this conversation",
                "row 7",
                "row 8",
                "row 9",
                "row 10",
                "row 11",
                "row 12 ",
            ]
        );
        assert_eq!((layout.scroll_top, layout.edited_rows), (Some(6), Some(6)));
        // The edited note's header is in the focus color; the others are not.
        assert_eq!(layout.rows[9][0].color, Some(THEME.active));
        assert_eq!(layout.rows[2][0].color, Some(THEME.accent_soft));
        // Moving the cursor to the top scrolls back.
        tab.session.editing = Some(NotesEditing {
            cursor: 0,
            scroll_top: 6,
        });
        assert_eq!(tab.layout(40, 16).scroll_top, Some(0));
    }

    #[test]
    fn a_sub_sessions_parent_note_follows_its_own_folded_until_opened() {
        let mut tab = Tab::new(steps(Some("Refresh"), &[]), line_note(""), session_note("mine"));
        tab.parent = Some(fold("Token storage's note", "keys live in the keychain", false));
        let rows = tab.render(50, 10);
        assert_eq!(rows[7..], ["mine", "▸ Token storage's note", ""]);
        tab.parent = Some(FoldInput {
            section: section(
                &["▾ Token storage's note · read-only", "▾ Token storage's note"],
                "keys live in the keychain",
                "",
            ),
            ..fold("Token storage's note", "", true)
        });
        let rows = tab.render(50, 12);
        assert_eq!(
            rows[7..],
            [
                "mine",
                "▾ Token storage's note · read-only",
                "keys live in the keychain",
                "",
                "",
            ]
        );
    }

    #[test]
    fn tiny_panes_and_stray_cursors_never_panic() {
        let mut tab = Tab::new(
            steps(Some("T"), &[("a", false), ("b", true)]),
            NotesSectionInput {
                editing: Some(NotesEditing {
                    cursor: 99,
                    scroll_top: 5,
                }),
                ..line_note("é中")
            },
            session_note("x"),
        );
        tab.focus = Some(NotePart::Line);
        for height in 0..12 {
            let layout = tab.layout(10, height);
            assert!(layout.rows.len() <= height.max(1), "{height}: {:?}", layout.rows);
        }
        tab.line.editing = Some(NotesEditing {
            cursor: 2,
            scroll_top: 0,
        });
        assert_eq!(tab.layout(10, 12).rows.len(), 12);
    }

    #[test]
    fn blank_rows_render_as_a_space_and_long_rows_are_cut() {
        assert_eq!(notes_row(&vec![plain("")]), vec![Seg::plain(" ")]);
        assert_eq!(notes_row(&Vec::new()), vec![Seg::plain(" ")]);
        let area = Rect::new(0, 0, 6, 2);
        let mut buf = Buffer::empty(area);
        render_notes_rows(&[vec![plain("abcdefghij")], vec![plain("")]], &mut buf, area);
        let first: String = (0..6).map(|x| buf[(x, 0)].symbol().to_owned()).collect();
        assert_eq!(first, "abcde…");
        let mut buf = Buffer::empty(area);
        render_notes_message("a longer message", &mut buf, area);
        let first: String = (0..6).map(|x| buf[(x, 0)].symbol().to_owned()).collect();
        assert_eq!(first, "a lon…");
        assert_eq!(buf[(0, 0)].fg, THEME.muted);
    }
}
