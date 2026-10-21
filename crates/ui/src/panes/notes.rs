//! Port of notesPane.tsx: the Notes tab's rows. The worktree's shared note above the session's own, each with a
//! header, separated by a rule; read mode shows rendered checklists from the top (`+N more lines` when cut), the
//! section being edited shows the raw Markdown with a cursor, scrolled to keep it visible. Layout is pure
//! ([`notes_layout`]) so it is unit-tested.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;

use deckhand_core::display::truncate;
use deckhand_core::jsstr::{js_len, js_trim};
use deckhand_core::notes::{
    NoteRowKind, SectionBudget, TaskLinkLookup, budget_sections, checklist_label, fit_first, fit_read_rows,
    note_read_rows, visible_text,
};
use deckhand_core::text_editor::{EditorState, scroll_top_for, wrapped_editor_lines};
use deckhand_protocol::data::NoteSection;

use crate::primitives::{Row, Seg, TextStyle, Wrap, draw_rows, fit_row, truncate_row};
use crate::theme::THEME;

/// TS: notesPane.tsx NotesSectionInput `editing`: the cursor (a byte offset) and the first row shown before this
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
    /// The section E / Ctrl+O open (header in bold).
    pub active: bool,
}

/// TS: notesPane.tsx NotesPart.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NotesPart {
    pub text: String,
    pub color: Option<Color>,
    pub bold: bool,
    pub dim: bool,
    pub inverse: bool,
}

/// TS: notesPane.tsx NotesRow.
pub type NotesRow = Vec<NotesPart>;

/// A part with only its text.
fn plain(text: impl Into<String>) -> NotesPart {
    NotesPart {
        text: text.into(),
        ..NotesPart::default()
    }
}

/// A part with a color.
fn colored(text: impl Into<String>, color: Color) -> NotesPart {
    NotesPart {
        text: text.into(),
        color: Some(color),
        ..NotesPart::default()
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
    let mut parts = vec![NotesPart {
        text: title,
        color: Some(if focused { THEME.active } else { THEME.accent_soft }),
        bold: focused || section.active,
        ..NotesPart::default()
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
pub fn need(section: &NotesSectionInput, width: usize, links: Option<TaskLinkLookup<'_>>) -> usize {
    if let Some(editing) = &section.editing {
        return wrapped_editor_lines(&editor_state(section, editing), width).lines.len();
    }
    if js_trim(&section.text).is_empty() {
        1
    } else {
        note_read_rows(&section.text, width, links).len()
    }
}

/// TS: notesPane.tsx body's result: the rows, and the edited section's first row for the next render.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NotesBody {
    pub rows: Vec<NotesRow>,
    pub scroll_top: Option<usize>,
}

/// TS: notesPane.tsx body: a section's body in `rows` rows.
pub fn body(section: &NotesSectionInput, width: usize, rows: usize, links: Option<TaskLinkLookup<'_>>) -> NotesBody {
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
                    row.push(NotesPart {
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
    let shown = fit_read_rows(note_read_rows(&section.text, width, links), rows)
        .into_iter()
        .map(|row| {
            vec![match row.kind {
                Some(NoteRowKind::Done) => colored(row.text, THEME.muted),
                Some(NoteRowKind::Link) => colored(row.text, THEME.accent_soft),
                Some(NoteRowKind::More) => NotesPart {
                    dim: true,
                    ..colored(row.text, THEME.muted)
                },
                Some(NoteRowKind::Heading) => NotesPart {
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

/// TS: notesPane.tsx notesLayout's argument.
#[derive(Clone, Copy)]
pub struct NotesLayoutInput<'a> {
    pub shared: Option<&'a NotesSectionInput>,
    pub session: Option<&'a NotesSectionInput>,
    pub width: usize,
    pub height: usize,
    pub focus: Option<NoteSection>,
    /// Whether each linked task is open, done or gone (`↗` lines say so).
    pub links: Option<TaskLinkLookup<'a>>,
}

/// TS: notesPane.tsx notesLayout's result.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NotesLayout {
    /// Exactly `height` rows (at least one).
    pub rows: Vec<NotesRow>,
    /// The edited section's first row, for the next render.
    pub scroll_top: Option<usize>,
    /// The rows each section's body got.
    pub bodies: SectionBudget,
}

/// TS: notesPane.tsx notesLayout: the tab's rows, exactly `height` of them: the shared section (when the session has
/// one), a rule, the session's (when it shows one: a session in a worktree has only the worktree's note).
pub fn notes_layout(input: &NotesLayoutInput<'_>) -> NotesLayout {
    let NotesLayoutInput {
        shared,
        session,
        width,
        height,
        focus,
        links,
    } = *input;
    let columns = width.max(1);
    // A session in a worktree shows only the worktree's note (no session section).
    let headers = if shared.is_some() && session.is_some() { 3 } else { 1 };
    let available = height.saturating_sub(headers);
    let bodies = match (shared, session) {
        (Some(_), None) => SectionBudget {
            shared: available,
            session: 0,
        },
        // Neither section: TS throws reading the missing session; there is nothing to lay out here.
        (shared, session) => budget_sections(
            available,
            shared.map(|shared| need(shared, columns, links)),
            session.map_or(1, |session| need(session, columns, links)),
            focus,
        ),
    };
    let mut rows: Vec<NotesRow> = Vec::new();
    let mut scroll_top = None;
    let add = |rows: &mut Vec<NotesRow>,
               scroll_top: &mut Option<usize>,
               section: &NotesSectionInput,
               which: NoteSection,
               count: usize| {
        rows.push(header(section, columns, focus == Some(which)));
        let shown = body(section, columns, count, links);
        if shown.scroll_top.is_some() {
            *scroll_top = shown.scroll_top;
        }
        let shown_rows = shown.rows.len();
        rows.extend(shown.rows);
        for _ in shown_rows..count {
            rows.push(vec![plain(" ")]);
        }
    };
    if let Some(shared) = shared {
        add(&mut rows, &mut scroll_top, shared, NoteSection::Shared, bodies.shared);
        if session.is_some() {
            rows.push(vec![colored("─".repeat(columns), THEME.border)]);
        }
    }
    if let Some(session) = session {
        add(
            &mut rows,
            &mut scroll_top,
            session,
            NoteSection::Session,
            bodies.session,
        );
    }
    rows.truncate(height.max(1));
    NotesLayout {
        rows,
        scroll_top,
        bodies,
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
    use deckhand_core::notes::TaskLinkState;
    use deckhand_testkit::{Case, replay};
    use serde_json::{Map, Value, json};

    // ── Ported TS tests ─────────────────────────────────────────────────────────────────────────────────────────

    fn section(titles: &[&str], text: &str, empty: &str) -> NotesSectionInput {
        NotesSectionInput {
            titles: titles.iter().map(|title| (*title).to_owned()).collect(),
            text: text.to_owned(),
            empty: empty.to_owned(),
            ..NotesSectionInput::default()
        }
    }

    /// TS tests/notes.test.ts `section(fields)`: title `Title`, empty text, `empty` as the empty line.
    fn basic(text: &str) -> NotesSectionInput {
        section(&["Title"], text, "empty")
    }

    fn input<'a>(
        shared: Option<&'a NotesSectionInput>,
        session: Option<&'a NotesSectionInput>,
        width: usize,
        height: usize,
        focus: Option<NoteSection>,
    ) -> NotesLayoutInput<'a> {
        NotesLayoutInput {
            shared,
            session,
            width,
            height,
            focus,
            links: None,
        }
    }

    /// TS tests/notes.test.ts renderLayout: the rows drawn through NotesRows, trailing spaces trimmed.
    fn render_layout(input: &NotesLayoutInput<'_>) -> Vec<String> {
        let layout = notes_layout(input);
        let area = Rect::new(
            0,
            0,
            u16::try_from(input.width).unwrap_or(u16::MAX),
            u16::try_from(input.height).unwrap_or(u16::MAX),
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

    fn joined(row: &NotesRow) -> String {
        row.iter().map(|part| part.text.as_str()).collect()
    }

    // covers: B-U4-01 Notes tab layout: worktree section, a rule, the session section; empty sections collapse; long ones end in +N more lines
    #[test]
    fn notes_tab_layout_sections_rule_and_empty_collapse() {
        let shared = section(
            &["Worktree · feat/x (shared by 2 sessions)", "Worktree · feat/x"],
            "- [ ] review\n- [x] merge",
            "No worktree notes · tab to add",
        );
        let lines: Vec<String> = (1..=8).map(|index| format!("line {index}")).collect();
        let own = section(
            &["This session · fix login", "This session"],
            &lines.join("\n"),
            "empty",
        );
        let rule = |width: usize| "─".repeat(width);
        assert_eq!(
            render_layout(&input(Some(&shared), Some(&own), 56, 9, None)),
            [
                "Worktree · feat/x (shared by 2 sessions) · ☐ 1 open",
                "☐ review",
                "☑ merge",
                &rule(56),
                "This session · fix login",
                "line 1",
                "line 2",
                "line 3",
                "+5 more lines",
            ]
        );
        // Narrow headers drop the "shared by" part before the count, then cut the title; an empty section is one
        // muted line.
        assert_eq!(
            render_layout(&input(Some(&shared), Some(&own), 40, 9, None))[0],
            "Worktree · feat/x · ☐ 1 open"
        );
        assert_eq!(
            render_layout(&input(Some(&shared), Some(&own), 24, 9, None))[0],
            "Worktree · f… · ☐ 1 open"
        );
        let emptied = NotesSectionInput {
            text: String::new(),
            ..shared.clone()
        };
        assert_eq!(
            render_layout(&input(Some(&emptied), Some(&own), 30, 6, None))[..4],
            [
                "Worktree · feat/x",
                "No worktree notes · tab to add",
                &rule(30),
                "This session · fix login"
            ]
        );
        // Without a shared note (worktree not ready), only the session's.
        let only = section(&["This session"], "only", "empty");
        assert_eq!(
            render_layout(&input(None, Some(&only), 20, 3, None)),
            ["This session", "only", ""]
        );
        // A session in a worktree: only the worktree's note, with the whole height.
        let worktree = section(&["Worktree · feat/x"], "plan\n- [ ] one", "empty");
        assert_eq!(
            render_layout(&input(Some(&worktree), None, 40, 4, None)),
            ["Worktree · feat/x · ☐ 1 open", "plan", "☐ one", ""]
        );
    }

    // covers: B-U4-02 Notes tab layout while editing: the focused section shows the raw text with the cursor and scrolls to keep it visible
    #[test]
    fn notes_tab_layout_while_editing_scrolls_to_the_cursor() {
        let lines: Vec<String> = (1..=12).map(|index| format!("row {index}")).collect();
        let text = lines.join("\n");
        let shared = section(
            &[
                "Worktree · main checkout (shared by 1 session)",
                "Worktree · main checkout",
            ],
            "shared\n- [ ] item",
            "empty",
        );
        let editing_session = NotesSectionInput {
            editing: Some(NotesEditing {
                cursor: text.len(),
                scroll_top: 0,
            }),
            ..section(&["This session"], &text, "empty")
        };
        let editing = notes_layout(&input(
            Some(&shared),
            Some(&editing_session),
            40,
            10,
            Some(NoteSection::Session),
        ));
        // 7 body rows: the shared section keeps its 2, the session section 5 (its need is more), scrolled to the end.
        assert_eq!(editing.scroll_top, Some(7));
        assert_eq!(editing.bodies, SectionBudget { shared: 2, session: 5 });
        let rows: Vec<String> = editing.rows.iter().map(joined).collect();
        assert_eq!(
            rows,
            [
                "Worktree · main checkout · ☐ 1 open",
                "shared",
                "☐ item",
                &"─".repeat(40),
                "This session",
                "row 8",
                "row 9",
                "row 10",
                "row 11",
                "row 12 ",
            ]
        );
        assert!(
            editing
                .rows
                .last()
                .and_then(|row| row.last())
                .is_some_and(|part| part.inverse)
        );
        // Moving the cursor to the top scrolls back; the raw checklist marker shows while editing.
        let editing_shared = NotesSectionInput {
            editing: Some(NotesEditing {
                cursor: 0,
                scroll_top: 0,
            }),
            ..shared.clone()
        };
        let plain_session = section(&["This session"], &text, "empty");
        let top = notes_layout(&input(
            Some(&editing_shared),
            Some(&plain_session),
            40,
            10,
            Some(NoteSection::Shared),
        ));
        let middle: Vec<String> = top.rows[1..3].iter().map(joined).collect();
        assert_eq!(middle, ["shared", "- [ ] item"]);
        assert_eq!(top.rows[1][0].text, "");
        assert!(top.rows[1][1].inverse);
    }

    #[test]
    fn a_session_without_either_section_has_no_rows() {
        let layout = notes_layout(&input(None, None, 40, 10, None));
        assert!(layout.rows.is_empty());
        assert_eq!(layout.scroll_top, None);
        // A stray cursor (past the end, inside a character) never reaches a slice.
        let stray = NotesSectionInput {
            editing: Some(NotesEditing {
                cursor: 99,
                scroll_top: 5,
            }),
            ..basic("é中")
        };
        let layout = notes_layout(&input(None, Some(&stray), 10, 4, None));
        assert_eq!(layout.rows.len(), 4);
        let inside = NotesSectionInput {
            editing: Some(NotesEditing {
                cursor: 2,
                scroll_top: 0,
            }),
            ..basic("é中")
        };
        assert_eq!(notes_layout(&input(None, Some(&inside), 10, 4, None)).rows.len(), 4);
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

    // ── Parity replay ───────────────────────────────────────────────────────────────────────────────────────────

    fn link_by_id(id: &str) -> TaskLinkState {
        match id.chars().next() {
            Some(first) if first.is_ascii_digit() => TaskLinkState::Done,
            Some('a'..='m') => TaskLinkState::Open,
            _ => TaskLinkState::Gone,
        }
    }

    fn section_arg(value: &Value) -> Result<NotesSectionInput, String> {
        let string = |key: &str| value.get(key).and_then(Value::as_str).map(str::to_owned);
        let count = |item: &Value, key: &str| -> usize {
            // A negative scrollTop is the same as 0: scrollTopFor ends at max(0, ...) either way.
            item.get(key)
                .and_then(Value::as_f64)
                .filter(|number| *number > 0.0)
                .map_or(0, |number| number as usize)
        };
        Ok(NotesSectionInput {
            titles: value
                .get("titles")
                .and_then(Value::as_array)
                .ok_or("section without titles")?
                .iter()
                .map(|title| title.as_str().map(str::to_owned).ok_or("title is not a string"))
                .collect::<Result<_, _>>()?,
            text: string("text").ok_or("section without text")?,
            empty: string("empty").ok_or("section without empty")?,
            flag: string("flag"),
            editing: value.get("editing").map(|editing| NotesEditing {
                cursor: count(editing, "cursor"),
                scroll_top: count(editing, "scrollTop"),
            }),
            active: value.get("active").and_then(Value::as_bool).unwrap_or(false),
        })
    }

    /// The Ink name of a theme color (what the fixtures record).
    fn ink_name(color: Color) -> String {
        match color {
            Color::Cyan => "cyan".to_owned(),
            Color::LightMagenta => "magentaBright".to_owned(),
            Color::DarkGray => "gray".to_owned(),
            Color::Yellow => "yellow".to_owned(),
            other => format!("{other:?}"),
        }
    }

    fn part_json(part: &NotesPart) -> Value {
        let mut object = Map::new();
        object.insert("text".to_owned(), json!(part.text));
        if let Some(color) = part.color {
            object.insert("color".to_owned(), json!(ink_name(color)));
        }
        for (key, on) in [("bold", part.bold), ("dim", part.dim), ("inverse", part.inverse)] {
            if on {
                object.insert(key.to_owned(), json!(true));
            }
        }
        Value::Object(object)
    }

    #[test]
    fn notes_layout_parity() {
        replay("notesPane/notesLayout", |case: &Case| {
            let raw = case.arg(0);
            let shared = raw.get("shared").map(section_arg).transpose()?;
            let session = raw.get("session").map(section_arg).transpose()?;
            let number = |key: &str| {
                raw.get(key)
                    .and_then(Value::as_f64)
                    .filter(|number| *number > 0.0)
                    .map_or(0, |number| number as usize)
            };
            let focus = match raw.get("focus").and_then(Value::as_str) {
                Some("shared") => Some(NoteSection::Shared),
                Some("session") => Some(NoteSection::Session),
                _ => None,
            };
            let open = |_: &str| TaskLinkState::Open;
            let done = |_: &str| TaskLinkState::Done;
            let gone = |_: &str| TaskLinkState::Gone;
            let links: Option<TaskLinkLookup<'_>> = match raw.get("links").and_then(|links| links.get("$fn")) {
                Some(Value::String(name)) => match name.as_str() {
                    "linkOpen" => Some(&open),
                    "linkDone" => Some(&done),
                    "linkGone" => Some(&gone),
                    "linkById" => Some(&link_by_id),
                    other => return Err(format!("unknown lookup {other}")),
                },
                _ => None,
            };
            let layout = notes_layout(&NotesLayoutInput {
                shared: shared.as_ref(),
                session: session.as_ref(),
                width: number("width"),
                height: number("height"),
                focus,
                links,
            });
            let mut rows: Vec<Vec<Value>> = layout
                .rows
                .iter()
                .map(|row| row.iter().map(part_json).collect())
                .collect();
            // TS leaves `bold` as `false` (not unset) on a header whose `active` is `false` and that is not focused
            // (`focused || section.active`); NotesPart has no unset state, so the recorded `false` is restored here.
            let raw_active = |key: &str| {
                raw.get(key)
                    .and_then(|section| section.get("active"))
                    .and_then(Value::as_bool)
            };
            let mut mark = |index: usize, active: Option<bool>, which: NoteSection| {
                if active == Some(false)
                    && focus != Some(which)
                    && let Some(part) = rows.get_mut(index).and_then(|row| row.first_mut())
                    && let Some(object) = part.as_object_mut()
                {
                    object.entry("bold").or_insert(json!(false));
                }
            };
            if shared.is_some() {
                mark(0, raw_active("shared"), NoteSection::Shared);
            }
            if session.is_some() {
                let at = if shared.is_some() { 2 + layout.bodies.shared } else { 0 };
                mark(at, raw_active("session"), NoteSection::Session);
            }
            let mut result = Map::new();
            result.insert("rows".to_owned(), json!(rows));
            if let Some(top) = layout.scroll_top {
                result.insert("scrollTop".to_owned(), json!(top));
            }
            result.insert(
                "bodies".to_owned(),
                json!({"shared": layout.bodies.shared, "session": layout.bodies.session}),
            );
            Ok::<_, String>(Value::Object(result))
        });
    }
}
