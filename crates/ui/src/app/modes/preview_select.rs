//! Preview focus: selecting the agent's screen with the mouse and copying it. Mouse reporting is on while you type
//! to the agent (so the wheel reaches it), which stops the terminal selecting text itself; Deckhand selects instead,
//! inside the Preview only, as tmux does in mouse mode. A left press in the agent's screen starts a selection, a drag
//! extends it from that cell to the pointer's (line by line, as a terminal selects), and the release copies the text
//! to the clipboard. A plain click, a key, or leaving focus clears it; so does the agent redrawing what it shows.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use deckhand_protocol::data::RightPaneTab;

use crate::app::state::{App, Mode};
use crate::panes::preview::render_screen;

/// A cell of the agent's screen as drawn in the Preview: its row and column from the body's top left (row first, so
/// cells order as text reads).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct ScreenCell {
    pub row: u16,
    pub col: u16,
}

/// A selection of the agent's screen, from the cell pressed to the cell the pointer is on (either may come first).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Selection {
    pub anchor: ScreenCell,
    pub head: ScreenCell,
    /// The pointer left the anchor's cell: the release copies (a plain click does not, and draws nothing).
    pub moved: bool,
    /// Set at the release: the text copied, while the highlight shows it. Once the screen under the highlight no
    /// longer reads this, the highlight goes.
    pub copied: Option<String>,
}

impl Selection {
    /// Its first and last cell, in reading order.
    pub fn range(&self) -> (ScreenCell, ScreenCell) {
        (self.anchor.min(self.head), self.anchor.max(self.head))
    }
}

/// What a mouse report says, for selecting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseAction {
    /// The left button went down.
    Press,
    /// The pointer moved with the left button down.
    Drag,
    /// The button came up.
    Release,
    /// Anything else (the wheel, other buttons, motion with no button down).
    Other,
}

/// One mouse report: its action at a terminal cell (zero-based column and row).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MouseReport {
    pub action: MouseAction,
    pub x: u16,
    pub y: u16,
}

/// The modifier bits of a report's button (Shift, Alt, Ctrl), which do not change what it selects.
const MODIFIER_BITS: u32 = 4 | 8 | 16;

/// A mouse report as framed by the input parser: SGR (`ESC [ < b ; x ; y M|m`, which preview focus asks for) or
/// legacy X10 (`ESC [ M` and three characters, from a terminal without SGR).
pub fn parse_mouse_report(sequence: &str) -> Option<MouseReport> {
    if let Some(body) = sequence.strip_prefix("\x1b[<") {
        let release = body.ends_with('m');
        let fields = body.strip_suffix(['M', 'm'])?;
        let mut numbers = fields.split(';').map(|field| field.parse::<u32>().ok());
        let (Some(Some(button)), Some(Some(x)), Some(Some(y)), None) =
            (numbers.next(), numbers.next(), numbers.next(), numbers.next())
        else {
            return None;
        };
        return Some(report(button, x, y, release));
    }
    let mut chars = sequence
        .strip_prefix("\x1b[M")?
        .chars()
        .map(|c| u32::from(c).checked_sub(32));
    let (Some(Some(button)), Some(Some(x)), Some(Some(y)), None) =
        (chars.next(), chars.next(), chars.next(), chars.next())
    else {
        return None;
    };
    // X10 reports a release as button 3, without saying which button.
    let release = button & !MODIFIER_BITS & (3 | 32 | 64 | 128) == 3;
    Some(report(button, x, y, release))
}

/// A report of `button` at 1-based (`x`, `y`), the button let go if `release`. SGR names the button let go (only the
/// left one ends a selection); X10 names none (button 3), so any release does.
fn report(button: u32, x: u32, y: u32, release: bool) -> MouseReport {
    let button = button & !MODIFIER_BITS;
    let motion = button & 32 != 0;
    let left = button & (3 | 64 | 128) == 0;
    let action = match (release, motion) {
        (true, _) if left || button & (3 | 64 | 128) == 3 => MouseAction::Release,
        (false, false) if left => MouseAction::Press,
        (false, true) if left => MouseAction::Drag,
        _ => MouseAction::Other,
    };
    let cell = |value: u32| u16::try_from(value.saturating_sub(1)).unwrap_or(u16::MAX);
    MouseReport {
        action,
        x: cell(x),
        y: cell(y),
    }
}

/// The cell of `body` (the agent's screen as drawn) at terminal cell (`x`, `y`): none outside it, unless `clamp`,
/// which takes the nearest cell of its edge (a drag that leaves the pane).
pub fn cell_at(body: Rect, x: u16, y: u16, clamp: bool) -> Option<ScreenCell> {
    if body.is_empty() {
        return None;
    }
    let inside = body.contains(ratatui::layout::Position { x, y });
    if !inside && !clamp {
        return None;
    }
    let col = x.clamp(body.left(), body.right() - 1) - body.x;
    let row = y.clamp(body.top(), body.bottom() - 1) - body.y;
    Some(ScreenCell { row, col })
}

/// The columns (first and last, inclusive) a selection from `start` to `end` covers in `row` of a screen `width`
/// wide: from the start's column on its row, to the end's on its row, whole rows between.
fn row_span(start: ScreenCell, end: ScreenCell, row: u16, width: u16) -> Option<(u16, u16)> {
    if row < start.row || row > end.row || width == 0 {
        return None;
    }
    let from = if row == start.row { start.col } else { 0 };
    let to = if row == end.row { end.col } else { width - 1 };
    Some((from, to.min(width - 1)))
}

/// The characters of `area`'s row `row` in `buf`, each with its first column (from `area.x`) and width: a wide
/// character's spacer cell belongs to it.
fn row_chars(buf: &Buffer, area: Rect, row: u16) -> Vec<(u16, u16, &str)> {
    let mut chars = Vec::new();
    let mut col = 0;
    while col < area.width {
        let symbol = buf[(area.x + col, area.y + row)].symbol();
        let width = u16::try_from(symbol.width()).unwrap_or(1).max(1);
        chars.push((col, width, symbol));
        col = col.saturating_add(width);
    }
    chars
}

/// Whether a character at `col`, `width` cells wide, has a cell in columns `from` to `to`.
fn overlaps(col: u16, width: u16, from: u16, to: u16) -> bool {
    col <= to && col + width > from
}

/// The text from `start` to `end` of what `buf` shows in `area`: each row's selected characters (a wide character
/// whose either half is selected, whole), trailing spaces trimmed, rows joined by line feeds.
pub fn selected_text(buf: &Buffer, area: Rect, start: ScreenCell, end: ScreenCell) -> String {
    let rows = start.row..=end.row.min(area.height.saturating_sub(1));
    let lines: Vec<String> = rows
        .filter_map(|row| {
            let (from, to) = row_span(start, end, row, area.width)?;
            let line: String = row_chars(buf, area, row)
                .into_iter()
                .filter(|(col, width, _)| overlaps(*col, *width, from, to))
                .map(|(_, _, symbol)| symbol)
                .collect();
            Some(line.trim_end_matches(' ').to_owned())
        })
        .collect();
    lines.join("\n")
}

/// Shows `start` to `end` of `area` in `buf` reversed (a character already reversed is shown plain), whole wide
/// characters, whole rows between the first and last.
pub fn highlight(buf: &mut Buffer, area: Rect, start: ScreenCell, end: ScreenCell) {
    for row in start.row..=end.row.min(area.height.saturating_sub(1)) {
        let Some((from, to)) = row_span(start, end, row, area.width) else {
            continue;
        };
        let spans: Vec<(u16, u16)> = row_chars(buf, area, row)
            .into_iter()
            .filter(|(col, width, _)| overlaps(*col, *width, from, to))
            .map(|(col, width, _)| (col, width))
            .collect();
        for (col, width) in spans {
            for x in col..(col + width).min(area.width) {
                buf[(area.x + x, area.y + row)].modifier.toggle(Modifier::REVERSED);
            }
        }
    }
}

/// Where the agent's screen is drawn, while a selection can be made in it: in preview focus, on the Preview tab, once
/// the styled screen arrived.
fn drawn_body(app: &App) -> Option<Rect> {
    let shown = app.mode == Mode::PreviewFocus
        && app.browse.active_tab == RightPaneTab::Preview
        && app.panes.preview.screen.is_some();
    app.preview_focus.body.filter(|body| shown && !body.is_empty())
}

/// The text a selection covers on the agent's screen as the Preview draws it now.
fn text_now(app: &App, body: Rect, selection: &Selection) -> String {
    let Some(screen) = app.panes.preview.screen.as_ref() else {
        return String::new();
    };
    let area = Rect::new(0, 0, body.width, body.height);
    let mut buf = Buffer::empty(area);
    render_screen(screen, &mut buf, area);
    let (start, end) = selection.range();
    selected_text(&buf, area, start, end)
}

/// A mouse report in preview focus: a left press in the agent's screen starts a selection (any press ends the last
/// one), a drag extends it (held to the pane's edge), and the release copies it. Nothing reaches the agent.
pub fn on_mouse_report(app: &mut App, sequence: &str) {
    let Some(report) = parse_mouse_report(sequence) else {
        return;
    };
    let body = drawn_body(app);
    match report.action {
        MouseAction::Press => {
            app.preview_focus.selection = body
                .and_then(|body| cell_at(body, report.x, report.y, false))
                .map(|cell| Selection {
                    anchor: cell,
                    head: cell,
                    ..Selection::default()
                });
        }
        MouseAction::Drag => {
            let (Some(body), Some(selection)) = (body, app.preview_focus.selection.as_mut()) else {
                return;
            };
            if selection.copied.is_some() {
                return;
            }
            if let Some(cell) = cell_at(body, report.x, report.y, true) {
                selection.head = cell;
                selection.moved |= cell != selection.anchor;
            }
        }
        MouseAction::Release => {
            if app
                .preview_focus
                .selection
                .as_ref()
                .is_some_and(|selection| selection.copied.is_none())
            {
                finish(app, body, report);
            }
        }
        MouseAction::Other => {}
    }
}

/// The release: a plain click clears the selection; a drag copies what is under it now.
fn finish(app: &mut App, body: Option<Rect>, report: MouseReport) {
    let (Some(body), Some(mut selection)) = (body, app.preview_focus.selection.take()) else {
        return;
    };
    if let Some(cell) = cell_at(body, report.x, report.y, true) {
        selection.moved |= cell != selection.anchor;
        selection.head = cell;
    }
    if !selection.moved {
        return;
    }
    let text = text_now(app, body, &selection);
    if text.trim().is_empty() {
        app.set_status(Some("Nothing copied: the selection is blank".to_owned()));
        return;
    }
    selection.copied = Some(text.clone());
    app.preview_focus.selection = Some(selection);
    let status = copied_status(&text);
    let copy = app.preview_focus.clipboard.copy(text);
    drop(app.dispatch().spawn(copy, move |app: &mut App, result| match result {
        Ok(()) => app.set_status(Some(status)),
        Err(error) => app.set_error(Some(error.to_string())),
    }));
}

/// What the status line says once `text` was copied: its lines, or for one line its characters.
pub fn copied_status(text: &str) -> String {
    let lines = text.split('\n').count();
    if lines > 1 {
        return format!("Copied {lines} lines");
    }
    let characters = text.graphemes(true).count();
    let plural = if characters == 1 { "" } else { "s" };
    format!("Copied {characters} character{plural}")
}

/// Highlights the selection over the agent's screen just drawn at `body`. A copied selection whose text the screen no
/// longer shows (the agent redrew it, or the preview scrolled) is dropped instead.
pub fn draw_selection(app: &mut App, buf: &mut Buffer, body: Rect) {
    if drawn_body(app).is_none() {
        return;
    }
    let Some(selection) = app.preview_focus.selection.as_ref().filter(|selection| selection.moved) else {
        return;
    };
    let (start, end) = selection.range();
    if let Some(copied) = &selection.copied
        && selected_text(buf, body, start, end) != *copied
    {
        app.preview_focus.selection = None;
        return;
    }
    highlight(buf, body, start, end);
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use deckhand_protocol::screen::PreviewScreen;
    use serde_json::json;

    use super::*;
    use crate::app::modes::browse::tests::{local, with_sessions};
    use crate::app::modes::preview_focus::on_raw_key;
    use crate::clipboard::{Clipboard, CopyError};
    use crate::input::{KeyEvent, StdinParser};

    /// The body the tests draw the agent's screen at.
    const BODY: Rect = Rect {
        x: 30,
        y: 4,
        width: 12,
        height: 5,
    };

    fn screen(lines: serde_json::Value) -> PreviewScreen {
        serde_json::from_value(json!({
            "lines": lines,
            "modes": {"bracketedPaste": false, "applicationCursorKeys": false, "mouseTracking": false},
        }))
        .unwrap_or_else(|error| panic!("{error}"))
    }

    /// Rows of plain text, each one segment from column 0 (its cells counted as the terminal shows them).
    fn plain(rows: &[&str]) -> PreviewScreen {
        let lines: Vec<serde_json::Value> = rows
            .iter()
            .map(|row| json!([[0, row.width(), row, "d", "d", ""]]))
            .collect();
        screen(json!(lines))
    }

    /// What `screen` reads from `start` to `end` drawn in [`BODY`].
    fn text(screen: &PreviewScreen, start: (u16, u16), end: (u16, u16)) -> String {
        let mut buf = Buffer::empty(Rect::new(0, 0, 80, 20));
        render_screen(screen, &mut buf, BODY);
        let cell = |(row, col)| ScreenCell { row, col };
        selected_text(&buf, BODY, cell(start), cell(end))
    }

    /// The copies a fake clipboard was given.
    type Copies = Rc<RefCell<Vec<String>>>;

    /// An app typing to s0, whose Preview draws `screen` at [`BODY`], copying to a fake clipboard that answers
    /// `result`.
    fn selecting(
        screen: PreviewScreen,
        result: Result<(), CopyError>,
    ) -> (
        App,
        tokio::sync::mpsc::UnboundedReceiver<crate::app::event_loop::Update<App>>,
        Copies,
    ) {
        let (mut app, rx) = with_sessions(1);
        app.set_mode(Mode::PreviewFocus);
        app.preview_focus.session_id = Some("s0".to_owned());
        app.panes.preview.screen = Some(screen);
        app.preview_focus.body = Some(BODY);
        let copies = Copies::default();
        let seen = Rc::clone(&copies);
        app.preview_focus.clipboard = Clipboard::new(move |text| {
            seen.borrow_mut().push(text);
            let result = result.clone();
            Box::pin(async move { result })
        });
        (app, rx, copies)
    }

    /// An SGR report at terminal cell (`x`, `y`), zero-based.
    fn sgr(button: u32, x: u16, y: u16, release: bool) -> String {
        let end = if release { 'm' } else { 'M' };
        format!("\x1b[<{button};{};{}{end}", x + 1, y + 1)
    }

    fn mouse(app: &mut App, sequence: &str) {
        on_raw_key(app, &KeyEvent::from_sequence(sequence.to_owned()));
    }

    /// Presses at body cell (`row`, `col`), drags through `through`, releases at the last.
    fn drag(app: &mut App, from: (u16, u16), through: &[(u16, u16)]) {
        let at = |(row, col): (u16, u16)| (BODY.x + col, BODY.y + row);
        let (x, y) = at(from);
        mouse(app, &sgr(0, x, y, false));
        for cell in through {
            let (x, y) = at(*cell);
            mouse(app, &sgr(32, x, y, false));
        }
        let (x, y) = at(*through.last().unwrap_or(&from));
        mouse(app, &sgr(0, x, y, true));
    }

    /// Draws the Preview's body as the frame does and returns it.
    fn drawn(app: &mut App) -> Buffer {
        let mut buf = Buffer::empty(Rect::new(0, 0, 80, 20));
        if let Some(screen) = app.panes.preview.screen.clone() {
            render_screen(&screen, &mut buf, BODY);
        }
        draw_selection(app, &mut buf, BODY);
        buf
    }

    fn reversed(buf: &Buffer, row: u16, col: u16) -> bool {
        buf[(BODY.x + col, BODY.y + row)].modifier.contains(Modifier::REVERSED)
    }

    #[test]
    fn reports_are_read_as_press_drag_and_release() {
        let report = |action, x, y| Some(MouseReport { action, x, y });
        assert_eq!(parse_mouse_report("\x1b[<0;5;3M"), report(MouseAction::Press, 4, 2));
        assert_eq!(parse_mouse_report("\x1b[<32;6;3M"), report(MouseAction::Drag, 5, 2));
        assert_eq!(parse_mouse_report("\x1b[<0;6;3m"), report(MouseAction::Release, 5, 2));
        // Shift, Alt or Ctrl held changes nothing.
        assert_eq!(parse_mouse_report("\x1b[<4;5;3M"), report(MouseAction::Press, 4, 2));
        assert_eq!(parse_mouse_report("\x1b[<48;5;3M"), report(MouseAction::Drag, 4, 2));
        // The wheel, other buttons and motion with no button down select nothing.
        for other in [
            "\x1b[<64;5;3M",
            "\x1b[<65;5;3M",
            "\x1b[<2;5;3M",
            "\x1b[<1;5;3M",
            "\x1b[<35;5;3M",
        ] {
            assert_eq!(
                parse_mouse_report(other).map(|report| report.action),
                Some(MouseAction::Other)
            );
        }
        assert_eq!(
            parse_mouse_report("\x1b[<64;5;3m").map(|r| r.action),
            Some(MouseAction::Other)
        );
        assert_eq!(
            parse_mouse_report("\x1b[<2;5;3m").map(|r| r.action),
            Some(MouseAction::Other)
        );
        // Legacy X10: the button, column and row offset by 32; a release is button 3.
        assert_eq!(parse_mouse_report("\x1b[M %#"), report(MouseAction::Press, 4, 2));
        assert_eq!(parse_mouse_report("\x1b[M@&#"), report(MouseAction::Drag, 5, 2));
        assert_eq!(parse_mouse_report("\x1b[M#&#"), report(MouseAction::Release, 5, 2));
        assert_eq!(
            parse_mouse_report("\x1b[M`%#").map(|r| r.action),
            Some(MouseAction::Other)
        );
        for broken in [
            "\x1b[<0;5M",
            "\x1b[<0;5;3;1M",
            "\x1b[<a;5;3M",
            "\x1b[<0;5;3",
            "\x1b[M !",
            "x",
            "\x1b[A",
        ] {
            assert_eq!(parse_mouse_report(broken), None, "{broken:?}");
        }
    }

    #[test]
    fn a_drag_arrives_as_one_report_per_event() {
        let chunk = StdinParser::new().push(b"\x1b[<0;5;3M\x1b[<32;9;4M\x1b[<0;9;4m");
        let sequences: Vec<&str> = chunk.events.iter().map(|event| event.sequence.as_str()).collect();
        assert_eq!(sequences, ["\x1b[<0;5;3M", "\x1b[<32;9;4M", "\x1b[<0;9;4m"]);
    }

    #[test]
    fn terminal_cells_map_to_the_screen_as_drawn() {
        // The body's top left is the screen's first cell; its far corner the last.
        assert_eq!(cell_at(BODY, 30, 4, false), Some(ScreenCell { row: 0, col: 0 }));
        assert_eq!(cell_at(BODY, 41, 8, false), Some(ScreenCell { row: 4, col: 11 }));
        // Outside (the header row above, the border, the sidebar): nothing, unless clamped to the nearest edge cell.
        for (x, y) in [(29, 4), (42, 4), (30, 3), (30, 9), (0, 0)] {
            assert_eq!(cell_at(BODY, x, y, false), None, "{x},{y}");
        }
        assert_eq!(cell_at(BODY, 0, 0, true), Some(ScreenCell { row: 0, col: 0 }));
        assert_eq!(cell_at(BODY, 79, 6, true), Some(ScreenCell { row: 2, col: 11 }));
        assert_eq!(cell_at(BODY, 35, 19, true), Some(ScreenCell { row: 4, col: 5 }));
        assert_eq!(cell_at(Rect::new(3, 3, 0, 4), 3, 3, true), None);
    }

    #[test]
    fn selected_text_runs_from_cell_to_cell_and_trims_each_line() {
        let screen = plain(&["first line", "second   ", "", "third one!"]);
        // One line, part of it; forwards or backwards the same.
        assert_eq!(text(&screen, (0, 2), (0, 6)), "rst l");
        assert_eq!(text(&screen, (3, 0), (3, 4)), "third");
        // Across lines: from the start column to the end of its row, whole rows between, the last up to its column.
        assert_eq!(text(&screen, (0, 6), (3, 4)), "line\nsecond\n\nthird");
        // Trailing spaces (the agent's and the empty cells past its text) are trimmed; spaces inside are kept.
        assert_eq!(text(&screen, (1, 0), (1, 11)), "second");
        assert_eq!(text(&screen, (3, 5), (3, 11)), " one!");
        assert_eq!(text(&screen, (2, 0), (2, 11)), "");
    }

    #[test]
    fn selected_text_keeps_wide_characters_whole() {
        // 日 takes columns 2–3, 本 4–5, 😀 6–7.
        let wide = plain(&["a 日本😀z"]);
        assert_eq!(
            text(&wide, (0, 0), (0, 11)),
            "a 日本😀z",
            "each spacer cell adds nothing"
        );
        // An edge on a wide character's second half takes the whole character.
        assert_eq!(text(&wide, (0, 3), (0, 4)), "日本");
        assert_eq!(text(&wide, (0, 5), (0, 5)), "本");
        assert_eq!(text(&wide, (0, 7), (0, 8)), "😀z");
        // A segment placed at its emulator column, after a styled one.
        let styled = screen(json!([[
            [0, 2, "> ", "p16:8", "d", ""],
            [2, 4, "日本", "p16:2", "d", "b"]
        ]]));
        assert_eq!(text(&styled, (0, 0), (0, 3)), "> 日");
    }

    #[test]
    fn the_highlight_covers_the_selection_and_whole_wide_characters() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 80, 20));
        render_screen(&plain(&["a 日本 b", "xyz"]), &mut buf, BODY);
        // From 本's second half on row 0 to column 1 of row 1.
        highlight(
            &mut buf,
            BODY,
            ScreenCell { row: 0, col: 5 },
            ScreenCell { row: 1, col: 1 },
        );
        let row0: Vec<bool> = (0..12).map(|col| reversed(&buf, 0, col)).collect();
        assert_eq!(&row0[..4], [false; 4]);
        assert!(
            row0[4..].iter().all(|on| *on),
            "本 whole, then to the row's end: {row0:?}"
        );
        let row1: Vec<bool> = (0..4).map(|col| reversed(&buf, 1, col)).collect();
        assert_eq!(row1, [true, true, false, false]);
        assert!(!reversed(&buf, 2, 0));
        // A character the agent showed reversed shows plain inside the selection.
        let mut buf = Buffer::empty(Rect::new(0, 0, 80, 20));
        render_screen(&screen(json!([[[0, 2, "ab", "d", "d", "r"]]])), &mut buf, BODY);
        highlight(
            &mut buf,
            BODY,
            ScreenCell { row: 0, col: 0 },
            ScreenCell { row: 0, col: 0 },
        );
        assert!(!reversed(&buf, 0, 0));
        assert!(reversed(&buf, 0, 1));
    }

    #[test]
    fn a_drag_copies_what_it_covers_and_says_so() {
        local(async {
            let (mut app, mut rx, copies) = selecting(plain(&["hello world", "second line"]), Ok(()));
            drag(&mut app, (0, 6), &[(0, 9), (1, 5)]);
            assert_eq!(*copies.borrow(), ["world\nsecond"]);
            // The status comes once the clipboard tool finished.
            assert_eq!(app.messages.status_message, None);
            let update = rx.recv().await.unwrap_or_else(|| panic!("the copy"));
            update(&mut app);
            assert_eq!(app.messages.status_message.as_deref(), Some("Copied 2 lines"));
            // The highlight stays on what was copied.
            let buf = drawn(&mut app);
            assert!(reversed(&buf, 0, 6) && reversed(&buf, 1, 5) && !reversed(&buf, 1, 6));
            assert!(app.preview_focus.selection.is_some());

            // Backwards on one line: the same text as forwards.
            drag(&mut app, (1, 10), &[(1, 7)]);
            assert_eq!(copies.borrow().last().map(String::as_str), Some("line"));
            rx.recv().await.unwrap_or_else(|| panic!("the copy"))(&mut app);
            assert_eq!(app.messages.status_message.as_deref(), Some("Copied 4 characters"));
        });
    }

    #[test]
    fn a_plain_click_clears_and_copies_nothing() {
        local(async {
            let (mut app, _rx, copies) = selecting(plain(&["hello world"]), Ok(()));
            drag(&mut app, (0, 0), &[(0, 4)]);
            assert!(app.preview_focus.selection.is_some());
            // Press and release on one cell (a drag report on that same cell too).
            drag(&mut app, (0, 2), &[(0, 2)]);
            assert_eq!(app.preview_focus.selection, None);
            assert_eq!(copies.borrow().len(), 1, "only the first drag copied");
            // A drag back to its own cell still copies that cell.
            drag(&mut app, (0, 2), &[(0, 3), (0, 2)]);
            assert_eq!(copies.borrow().last().map(String::as_str), Some("l"));
        });
    }

    #[test]
    fn a_blank_selection_copies_nothing_and_says_so() {
        local(async {
            let (mut app, _rx, copies) = selecting(plain(&["hi", "", ""]), Ok(()));
            drag(&mut app, (1, 0), &[(2, 8)]);
            assert!(copies.borrow().is_empty(), "the tool is not run");
            assert_eq!(app.preview_focus.selection, None);
            assert_eq!(
                app.messages.status_message.as_deref(),
                Some("Nothing copied: the selection is blank")
            );
        });
    }

    #[test]
    fn a_failed_copy_says_why() {
        local(async {
            let (mut app, mut rx, _copies) = selecting(plain(&["hello"]), Err(CopyError::NoTool(vec!["pbcopy"])));
            drag(&mut app, (0, 0), &[(0, 4)]);
            rx.recv().await.unwrap_or_else(|| panic!("the copy"))(&mut app);
            assert_eq!(
                app.messages.error.as_deref(),
                Some("Could not copy: pbcopy was not found")
            );
            assert_eq!(app.messages.status_message, None);
        });
    }

    #[test]
    fn copied_statuses() {
        assert_eq!(copied_status("a\nb\nc"), "Copied 3 lines");
        assert_eq!(copied_status("a\n"), "Copied 2 lines");
        assert_eq!(copied_status(&"x".repeat(42)), "Copied 42 characters");
        assert_eq!(copied_status("日"), "Copied 1 character");
        assert_eq!(copied_status("👍🏽!"), "Copied 2 characters");
    }

    #[test]
    fn a_key_or_leaving_focus_clears_the_selection() {
        local(async {
            let (mut app, _rx, _copies) = selecting(plain(&["hello world"]), Ok(()));
            let _daemon = crate::app::modes::browse::tests::Daemon::connect(&mut app);
            drag(&mut app, (0, 0), &[(0, 4)]);
            mouse(&mut app, "z");
            assert_eq!(app.preview_focus.selection, None);
            // Mid-drag too: the release then copies nothing.
            mouse(&mut app, &sgr(0, BODY.x, BODY.y, false));
            mouse(&mut app, &sgr(32, BODY.x + 3, BODY.y, false));
            mouse(&mut app, "z");
            mouse(&mut app, &sgr(0, BODY.x + 3, BODY.y, true));
            assert_eq!(app.preview_focus.selection, None);

            drag(&mut app, (0, 0), &[(0, 4)]);
            crate::app::modes::preview_focus::leave_focus(&mut app, None);
            assert_eq!(app.preview_focus.selection, None);
        });
    }

    #[test]
    fn a_press_restarts_and_a_press_outside_starts_nothing() {
        local(async {
            let (mut app, _rx, copies) = selecting(plain(&["hello world", "again"]), Ok(()));
            // A release lost outside the window: the next press starts over from its own cell.
            mouse(&mut app, &sgr(0, BODY.x, BODY.y, false));
            mouse(&mut app, &sgr(32, BODY.x + 4, BODY.y, false));
            drag(&mut app, (1, 0), &[(1, 2)]);
            assert_eq!(*copies.borrow(), ["aga"]);
            // A press outside the agent's screen (the header row, the sidebar) starts nothing, and drags after it
            // select nothing.
            mouse(&mut app, &sgr(0, BODY.x, BODY.y - 1, false));
            assert_eq!(app.preview_focus.selection, None);
            mouse(&mut app, &sgr(32, BODY.x + 4, BODY.y, false));
            mouse(&mut app, &sgr(0, BODY.x + 4, BODY.y, true));
            assert_eq!(app.preview_focus.selection, None);
            assert_eq!(copies.borrow().len(), 1);
            // A drag that leaves the pane holds to its edge: right of it is the end of the row, below it the last row.
            let (x, y) = (BODY.right() + 10, BODY.y);
            mouse(&mut app, &sgr(0, BODY.x + 6, BODY.y, false));
            mouse(&mut app, &sgr(32, x, y, false));
            mouse(&mut app, &sgr(0, x, y, true));
            assert_eq!(copies.borrow().last().map(String::as_str), Some("world"));
            mouse(&mut app, &sgr(0, BODY.x + 6, BODY.y, false));
            mouse(&mut app, &sgr(32, 0, 19, false));
            mouse(&mut app, &sgr(0, 0, 19, true));
            assert_eq!(copies.borrow().last().map(String::as_str), Some("world\nagain\n\n\n"));
        });
    }

    #[test]
    fn the_highlight_goes_once_the_agent_redraws_under_it() {
        local(async {
            let (mut app, _rx, _copies) = selecting(plain(&["hello world", "next"]), Ok(()));
            drag(&mut app, (0, 0), &[(0, 4)]);
            // Mid-drag the highlight follows the pointer whatever streams in; the release copies what is there then.
            mouse(&mut app, &sgr(0, BODY.x, BODY.y + 1, false));
            mouse(&mut app, &sgr(32, BODY.x + 3, BODY.y + 1, false));
            app.panes.preview.screen = Some(plain(&["hello world", "new text"]));
            assert!(reversed(&drawn(&mut app), 1, 3));
            mouse(&mut app, &sgr(0, BODY.x + 3, BODY.y + 1, true));
            assert_eq!(
                app.preview_focus.selection.as_ref().and_then(|s| s.copied.as_deref()),
                Some("new"),
                "trailing spaces trimmed"
            );
            // Output elsewhere keeps it.
            app.panes.preview.screen = Some(plain(&["changed", "new text", "more"]));
            assert!(reversed(&drawn(&mut app), 1, 0));
            // Output under it (or the preview scrolling) drops it.
            app.panes.preview.screen = Some(plain(&["changed", "more", "lines"]));
            assert!(!reversed(&drawn(&mut app), 1, 0));
            assert_eq!(app.preview_focus.selection, None);
        });
    }

    #[test]
    fn nothing_is_selected_outside_preview_focus_or_before_the_screen_arrives() {
        local(async {
            let (mut app, _rx, copies) = selecting(plain(&["hello"]), Ok(()));
            app.panes.preview.screen = None;
            drag(&mut app, (0, 0), &[(0, 4)]);
            assert_eq!(app.preview_focus.selection, None);
            app.panes.preview.screen = Some(plain(&["hello"]));
            app.set_active_tab(RightPaneTab::Terminal);
            drag(&mut app, (0, 0), &[(0, 4)]);
            assert_eq!(app.preview_focus.selection, None);
            assert!(copies.borrow().is_empty());
        });
    }

    #[test]
    fn the_frame_highlights_the_selection_in_the_preview() {
        use crate::app::event_loop::UiModel;
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        local(async {
            let (mut app, _rx, copies) = selecting(plain(&["hello world"]), Ok(()));
            app.preview_focus.body = None;
            let size = app.browse.terminal_size;
            let mut terminal =
                Terminal::new(TestBackend::new(size.cols, size.rows)).unwrap_or_else(|error| panic!("{error}"));
            let mut frame = |app: &mut App| {
                terminal
                    .draw(|frame| app.render(frame))
                    .unwrap_or_else(|error| panic!("{error}"));
                terminal.backend().buffer().clone()
            };
            frame(&mut app);
            // The agent's screen is drawn below the Preview's header at the PTY's size.
            let body = app.preview_focus.body.unwrap_or_else(|| panic!("the Preview's body"));
            let layout = app.layout();
            assert_eq!((body.width, body.height), (layout.preview_cols, layout.preview_rows));
            mouse(&mut app, &sgr(0, body.x + 6, body.y, false));
            mouse(&mut app, &sgr(32, body.x + 10, body.y, false));
            mouse(&mut app, &sgr(0, body.x + 10, body.y, true));
            assert_eq!(*copies.borrow(), ["world"]);
            let buf = frame(&mut app);
            let cell = |col: u16| &buf[(body.x + col, body.y)];
            assert_eq!(cell(6).symbol(), "w");
            let on: Vec<bool> = (4..12)
                .map(|col| cell(col).modifier.contains(Modifier::REVERSED))
                .collect();
            assert_eq!(on, [false, false, true, true, true, true, true, false]);
        });
    }

    #[test]
    fn selection_reports_never_reach_the_agent() {
        local(async {
            let (mut app, _rx, _copies) = selecting(plain(&["hello world"]), Ok(()));
            let mut daemon = crate::app::modes::browse::tests::Daemon::connect(&mut app);
            drag(&mut app, (0, 0), &[(0, 3), (0, 6)]);
            mouse(&mut app, "\x1b[M %%");
            mouse(&mut app, "z");
            assert_eq!(daemon.next().await["data"], "z");
        });
    }
}
