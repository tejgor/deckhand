//! Not in the TS: a Deckhand status line on the top row of an attached agent, with a rule under it.
//!
//! The agent's PTY gets [`HEIGHT`] rows less than the outer terminal and a scroll region keeps its output below them,
//! where Deckhand draws its own line (the session, how many other sessions wait on you, the detach key) and a rule that
//! separates it from the agent. The top costs nothing: every agent attach uses Deckhand's alternate screen (Claude, Pi
//! and Codex all draw fullscreen), which has no scrollback to lose (most terminals, xterm.js among them, only keep lines
//! that scroll out of a region starting at the top row). The agent's output still goes straight to the terminal; on
//! the way it is adjusted so it cannot reach those rows:
//! - scroll regions are clamped to the agent's rows (a reset means all of them) and moved down by [`HEIGHT`];
//! - absolute row moves (`CUP`, `HVP`, `VPA`) are clamped to the agent's rows and moved down by [`HEIGHT`] (and the
//!   terminal's cursor position reports up by as much, on their way back to the agent);
//! - after anything that erases or replaces those rows (erasing the display, a full reset, switching screens) they are
//!   drawn again, and a full reset also restores the scroll region.
//!
//! Redrawing must leave the cursor and colours where the agent had them. The agent may be using the terminal's one
//! saved-cursor slot itself, so instead of saving the cursor the agent's output is mirrored into a small emulator
//! (`vt100`, in the agent's own coordinates) that knows the cursor position and drawing attributes to restore. Redraws
//! wait for the end of the agent's synchronized update, and are wrapped in one themselves.

use std::fmt::Write as _;

use indexmap::IndexMap;
use unicode_width::UnicodeWidthChar;

use deckhand_core::agent_signals::needs_attention;
use deckhand_protocol::data::{AttentionState, SessionRecord, SessionStatus};

/// The rows Deckhand takes at the top: the line, and the rule under it.
const HEIGHT: u16 = 2;

/// Fewer outer rows than this leaves the agent too little room: no status line.
const MIN_ROWS: u16 = HEIGHT + 3;

/// What the status line knows about one session.
#[derive(Clone, Debug, PartialEq, Eq)]
struct SessionSummary {
    attention: Option<AttentionState>,
    running: bool,
    archived: bool,
}

impl SessionSummary {
    fn of(session: &SessionRecord) -> Self {
        SessionSummary {
            attention: session.attention.as_ref().map(|attention| attention.state),
            running: session.status != SessionStatus::Exited,
            archived: session.archived_at.is_some(),
        }
    }

    fn waiting(&self) -> bool {
        self.running && !self.archived && needs_attention(self.attention)
    }
}

/// The status line of one attach: the output adjuster, the cursor-tracking mirror and the line's content.
pub struct StatusLine {
    session_id: String,
    title: String,
    /// The outer terminal's size; the agent gets `rows - HEIGHT`.
    rows: u16,
    cols: u16,
    /// The agent's screen, in its own coordinates.
    mirror: vt100::Parser,
    /// An escape sequence cut off at the end of the last chunk.
    carry: String,
    /// Inside the agent's synchronized update (`CSI ? 2026 h` … `l`).
    in_sync: bool,
    /// A redraw is due once the agent's synchronized update ends.
    redraw_due: bool,
    /// Cursor position reports the agent asked for (`CSI 6 n`) whose answers have not come back yet.
    pending_reports: u32,
    sessions: IndexMap<String, SessionSummary>,
}

impl StatusLine {
    /// A status line for an outer terminal of `rows` × `cols`, or `None` when it is too small for one.
    pub fn new(session_id: &str, title: &str, rows: i64, cols: i64) -> Option<Self> {
        let (rows, cols) = (clamp_u16(rows), clamp_u16(cols));
        if rows < MIN_ROWS || cols == 0 {
            return None;
        }
        Some(StatusLine {
            session_id: session_id.to_owned(),
            title: title.to_owned(),
            rows,
            cols,
            mirror: vt100::Parser::new(rows - HEIGHT, cols, 0),
            carry: String::new(),
            in_sync: false,
            redraw_due: false,
            pending_reports: 0,
            sessions: IndexMap::new(),
        })
    }

    /// The rows the agent's PTY gets.
    pub fn agent_rows(&self) -> i64 {
        i64::from(self.rows - HEIGHT)
    }

    /// The outer terminal was resized: returns what to write (the new scroll region and the line).
    pub fn resize(&mut self, rows: i64, cols: i64) -> String {
        let (rows, cols) = (clamp_u16(rows).max(MIN_ROWS), clamp_u16(cols).max(1));
        self.rows = rows;
        self.cols = cols;
        self.mirror.set_size(rows - HEIGHT, cols);
        self.setup()
    }

    /// Reserves the line's rows (scroll region) and draws them: written once attached, and after a resize.
    pub fn setup(&self) -> String {
        // Setting a scroll region homes the cursor, so the redraw's restore puts it back where the mirror has it.
        let mut out = self.region();
        out.push_str(&self.draw());
        out
    }

    /// Replaces what the line knows about the repository's sessions (the subscribe answer).
    pub fn set_sessions(&mut self, sessions: &[SessionRecord]) -> String {
        self.sessions = sessions
            .iter()
            .map(|session| (session.id.clone(), SessionSummary::of(session)))
            .collect();
        self.redraw_soon()
    }

    /// A `session-updated` event: the line is drawn again when what it shows changed.
    pub fn session_updated(&mut self, session: &SessionRecord) -> String {
        let summary = SessionSummary::of(session);
        let title_changed = session.id == self.session_id && session.title != self.title;
        if title_changed {
            session.title.clone_into(&mut self.title);
        }
        let before = self.sessions.insert(session.id.clone(), summary.clone());
        if title_changed || before != Some(summary) {
            return self.redraw_soon();
        }
        String::new()
    }

    /// A `session-removed` event.
    pub fn session_removed(&mut self, session_id: &str) -> String {
        if self.sessions.shift_remove(session_id).is_some() {
            return self.redraw_soon();
        }
        String::new()
    }

    /// The agent's output on its way to the terminal: adjusted (see the module docs), mirrored, and followed by a
    /// redraw when it erased the line.
    pub fn filter_output(&mut self, data: &str) -> String {
        let mut text = std::mem::take(&mut self.carry);
        text.push_str(data);
        let mut agent = String::with_capacity(text.len());
        let mut out = String::with_capacity(text.len() + 16);
        let mut redraw = false;
        let mut reset = false;
        let mut rest = text.as_str();
        while let Some(at) = rest.find('\x1b') {
            agent.push_str(&rest[..at]);
            out.push_str(&rest[..at]);
            let sequence = &rest[at..];
            let Some(len) = escape_len(sequence) else {
                // Cut off: keep it for the next chunk.
                self.carry = sequence.to_owned();
                rest = "";
                break;
            };
            let (escape, after) = sequence.split_at(len);
            match self.adjust(escape) {
                Adjusted::Keep => {
                    agent.push_str(escape);
                    out.push_str(escape);
                }
                Adjusted::Replace { agent: own, terminal } => {
                    agent.push_str(&own);
                    out.push_str(&terminal);
                }
                Adjusted::Redraw => {
                    agent.push_str(escape);
                    out.push_str(escape);
                    redraw = true;
                }
                Adjusted::Reset => {
                    agent.push_str(escape);
                    out.push_str(escape);
                    reset = true;
                }
            }
            rest = after;
        }
        agent.push_str(rest);
        out.push_str(rest);
        self.mirror.process(agent.as_bytes());
        if reset {
            out.push_str(&self.region());
            // The region homes the cursor; the redraw restores the mirror's position.
            redraw = true;
        }
        if redraw {
            self.redraw_due = true;
        }
        if self.redraw_due && !self.in_sync {
            out.push_str(&self.draw());
            self.redraw_due = false;
        }
        out
    }

    /// Input on its way to the agent: SGR mouse reports on the line or the rule are dropped and the rest moved up by
    /// [`HEIGHT`], as are the rows of the cursor position reports the agent asked for.
    pub fn filter_input(&mut self, data: &str) -> String {
        let mut out = String::with_capacity(data.len());
        let mut rest = data;
        while let Some(at) = rest.find("\x1b[") {
            out.push_str(&rest[..at]);
            let sequence = &rest[at..];
            if let Some((len, row)) = sgr_mouse(sequence) {
                if row > u32::from(HEIGHT) {
                    out.push_str(&shift_mouse_row(&sequence[..len], row - u32::from(HEIGHT)));
                }
                rest = &sequence[len..];
                continue;
            }
            if self.pending_reports > 0
                && let Some((len, row, col)) = cursor_report(sequence)
            {
                self.pending_reports -= 1;
                let _ = write!(out, "\x1b[{};{col}R", row.saturating_sub(u32::from(HEIGHT)).max(1));
                rest = &sequence[len..];
                continue;
            }
            out.push_str("\x1b[");
            rest = &sequence[2..];
        }
        out.push_str(rest);
        out
    }

    /// Clears the line and the rule (written before handing the terminal back).
    pub fn teardown(&self) -> String {
        String::from("\x1b[r\x1b[0m\x1b[1;1H\x1b[2K\x1b[2;1H\x1b[2K\x1b[1;1H")
    }

    /// The scroll region that keeps the agent below the line and the rule.
    fn region(&self) -> String {
        format!("\x1b[{};{}r", HEIGHT + 1, self.rows)
    }

    fn redraw_soon(&mut self) -> String {
        if self.in_sync {
            self.redraw_due = true;
            return String::new();
        }
        self.draw()
    }

    fn adjust(&mut self, escape: &str) -> Adjusted {
        let agent_rows = u32::from(self.rows - HEIGHT);
        let offset = u32::from(HEIGHT);
        let bytes = escape.as_bytes();
        if bytes.len() == 2 && bytes[1] == b'c' {
            return Adjusted::Reset;
        }
        if bytes.len() < 3 || bytes[1] != b'[' {
            return Adjusted::Keep;
        }
        let body = &escape[2..escape.len() - 1];
        let last = bytes[bytes.len() - 1];
        if let Some(modes) = body.strip_prefix('?') {
            let modes = modes.split(';').filter_map(|mode| mode.parse::<u32>().ok());
            let mut redraw = false;
            for mode in modes {
                match (mode, last) {
                    (2026, b'h') => self.in_sync = true,
                    (2026, b'l') => self.in_sync = false,
                    (47 | 1047 | 1049, b'h' | b'l') => redraw = true,
                    _ => {}
                }
            }
            return if redraw { Adjusted::Redraw } else { Adjusted::Keep };
        }
        if body.bytes().any(|b| !(b.is_ascii_digit() || b == b';')) {
            return Adjusted::Keep;
        }
        let params: Vec<Option<u32>> = body
            .split(';')
            .map(|param| if param.is_empty() { None } else { param.parse().ok() })
            .collect();
        let param = |index: usize| params.get(index).copied().flatten();
        // A row the agent names, as the terminal would take it on the agent's screen (0 means 1).
        let row = |index: usize| param(index).unwrap_or(1).clamp(1, agent_rows);
        match last {
            // DECSTBM: never past the agent's rows (a reset means all of them).
            b'r' => {
                let first = row(0);
                let end = param(1).unwrap_or(agent_rows).clamp(1, agent_rows);
                Adjusted::Replace {
                    agent: format!("\x1b[{first};{end}r"),
                    terminal: format!("\x1b[{};{}r", first + offset, end + offset),
                }
            }
            // CUP / HVP: the row clamped (and moved), the column kept.
            b'H' | b'f' => {
                let first = row(0);
                let col = params
                    .get(1)
                    .copied()
                    .flatten()
                    .map(|col| col.to_string())
                    .unwrap_or_default();
                let end = last as char;
                Adjusted::Replace {
                    agent: format!("\x1b[{first};{col}{end}"),
                    terminal: format!("\x1b[{};{col}{end}", first + offset),
                }
            }
            // VPA.
            b'd' => {
                let first = row(0);
                Adjusted::Replace {
                    agent: format!("\x1b[{first}d"),
                    terminal: format!("\x1b[{}d", first + offset),
                }
            }
            // ED: 1 (start to cursor), 2 (all) and 3 (all and scrollback) reach the line and the rule; 0 (cursor to
            // end) never does.
            b'J' => match param(0).unwrap_or(0) {
                1..=3 => Adjusted::Redraw,
                _ => Adjusted::Keep,
            },
            // DSR 6: the terminal answers with the cursor position, which the input side moves back.
            b'n' if param(0) == Some(6) => {
                self.pending_reports += 1;
                Adjusted::Keep
            }
            _ => Adjusted::Keep,
        }
    }

    /// The line and the rule under it, drawn on their rows, then the agent's cursor and drawing attributes put back.
    fn draw(&self) -> String {
        let waiting = self
            .sessions
            .iter()
            .filter(|(id, summary)| **id != self.session_id && summary.waiting())
            .count();
        let own = self
            .sessions
            .get(&self.session_id)
            .and_then(|summary| summary.attention);
        let mut out = String::from("\x1b[?2026h");
        out.push_str("\x1b[1;1H\x1b[0m\x1b[48;5;236m\x1b[38;5;250m");
        let mut line = Line::new(usize::from(self.cols));
        line.push("\x1b[1;38;5;212m", " deckhand ");
        line.push("\x1b[22;38;5;245m", "│ ");
        line.push("\x1b[38;5;252m", &self.title);
        if let Some(state) = own {
            line.push("\x1b[38;5;245m", " · ");
            line.push(state_colour(state), state_label(state));
        }
        line.push("\x1b[38;5;245m", " │ ");
        if waiting > 0 {
            let plural = if waiting == 1 { "" } else { "s" };
            line.push(
                "\x1b[1;38;5;221m",
                &format!("! {waiting} other session{plural} waiting"),
            );
            line.push("\x1b[22m", "");
        } else {
            line.push("\x1b[38;5;245m", "nothing else waiting");
        }
        line.push("\x1b[38;5;245m", " │ Ctrl+] detach");
        out.push_str(&line.finish());
        out.push_str("\x1b[0m\x1b[2;1H");
        // Like the line, the rule leaves the last cell empty.
        let mut rule = Line::new(usize::from(self.cols));
        rule.push("\x1b[38;5;239m", &"─".repeat(usize::from(self.cols - 1)));
        out.push_str(&rule.finish());
        out.push_str("\x1b[0m");
        let screen = self.mirror.screen();
        let (row, col) = screen.cursor_position();
        let col = col.min(self.cols.saturating_sub(1));
        let _ = write!(out, "\x1b[{};{}H", u32::from(row + HEIGHT) + 1, u32::from(col) + 1);
        out.push_str(&String::from_utf8_lossy(&screen.attributes_formatted()));
        out.push_str("\x1b[?2026l");
        out
    }
}

/// What to do with one escape sequence of the agent's output.
enum Adjusted {
    Keep,
    /// Written differently: `agent` for the mirror (the agent's coordinates), `terminal` for the terminal.
    Replace {
        agent: String,
        terminal: String,
    },
    /// Keep it, then draw the line again (it was erased or the screen switched).
    Redraw,
    /// A full reset: keep it, restore the scroll region, draw the line again.
    Reset,
}

/// A status line being filled up to the terminal's width (the rest padded with the background).
struct Line {
    text: String,
    width: usize,
    room: usize,
}

impl Line {
    fn new(room: usize) -> Self {
        Line {
            text: String::new(),
            width: 0,
            room,
        }
    }

    fn push(&mut self, style: &str, text: &str) {
        self.text.push_str(style);
        for ch in text.chars() {
            let ch = if ch.is_control() { ' ' } else { ch };
            let width = ch.width().unwrap_or(0);
            if self.width + width > self.room {
                return;
            }
            self.text.push(ch);
            self.width += width;
        }
    }

    fn finish(mut self) -> String {
        // The last cell is left empty so writing the line never sets a pending wrap.
        let pad = self.room.saturating_sub(self.width + 1);
        self.text.extend(std::iter::repeat_n(' ', pad));
        self.text
    }
}

fn state_label(state: AttentionState) -> &'static str {
    match state {
        AttentionState::Working => "working",
        AttentionState::NeedsInput => "needs input",
        AttentionState::ResponseEnded => "done",
        AttentionState::Failed => "failed",
        AttentionState::Limited => "rate limited",
        AttentionState::Unknown => "idle",
    }
}

fn state_colour(state: AttentionState) -> &'static str {
    match state {
        AttentionState::Working => "\x1b[38;5;117m",
        AttentionState::NeedsInput | AttentionState::Limited => "\x1b[38;5;221m",
        AttentionState::ResponseEnded => "\x1b[38;5;114m",
        AttentionState::Failed => "\x1b[38;5;203m",
        AttentionState::Unknown => "\x1b[38;5;245m",
    }
}

fn clamp_u16(value: i64) -> u16 {
    u16::try_from(value.max(0)).unwrap_or(u16::MAX)
}

/// The length of the escape sequence `text` starts with (`text` starts with ESC), or `None` when it is cut off.
fn escape_len(text: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    let second = *bytes.get(1)?;
    match second {
        b'[' => {
            // CSI: parameter bytes 0x30–0x3F, intermediates 0x20–0x2F, a final byte 0x40–0x7E.
            for (index, byte) in bytes.iter().enumerate().skip(2) {
                match byte {
                    0x20..=0x3f => {}
                    0x40..=0x7e => return Some(index + 1),
                    // Not a CSI after all: pass the ESC [ on and look at the rest as text.
                    _ => return Some(2),
                }
            }
            None
        }
        // String sequences (OSC, DCS, SOS, PM, APC) end with BEL (OSC) or ESC \.
        b']' | b'P' | b'X' | b'^' | b'_' => {
            let body = &text[2..];
            let bel = if second == b']' {
                body.find('\x07').map(|at| at + 3)
            } else {
                None
            };
            let st = body.find("\x1b\\").map(|at| at + 4);
            match (bel, st) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (a, b) => a.or(b),
            }
        }
        // ESC with intermediates (charset designation and the like), then a final byte.
        0x20..=0x2f => {
            for (index, byte) in bytes.iter().enumerate().skip(2) {
                if !(0x20..=0x2f).contains(byte) {
                    return Some(index + 1);
                }
            }
            None
        }
        _ => Some(1 + text[1..].chars().next().map_or(1, char::len_utf8)),
    }
}

/// An SGR mouse report at the start of `text` (`ESC [ < b ; x ; y M|m`): its length and row.
fn sgr_mouse(text: &str) -> Option<(usize, u32)> {
    let body = text.strip_prefix("\x1b[<")?;
    let end = body.find(['M', 'm'])?;
    let mut fields = body[..end].split(';');
    let (_button, _col, row) = (fields.next()?, fields.next()?, fields.next()?);
    if fields.next().is_some() {
        return None;
    }
    Some((3 + end + 1, row.parse().ok()?))
}

/// `report` (an SGR mouse report) with its row replaced by `row`.
fn shift_mouse_row(report: &str, row: u32) -> String {
    let final_byte = &report[report.len() - 1..];
    let body = &report[3..report.len() - 1];
    let mut fields = body.split(';');
    let (button, col) = (fields.next().unwrap_or("0"), fields.next().unwrap_or("1"));
    format!("\x1b[<{button};{col};{row}{final_byte}")
}

/// A cursor position report at the start of `text` (`ESC [ row ; col R`): its length, row and column.
fn cursor_report(text: &str) -> Option<(usize, u32, u32)> {
    let body = text.strip_prefix("\x1b[")?;
    let end = body.find('R')?;
    let (row, col) = body[..end].split_once(';')?;
    Some((2 + end + 1, row.parse().ok()?, col.parse().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line() -> StatusLine {
        StatusLine::new("s", "fix-login", 6, 40).unwrap()
    }

    /// The outer terminal after `writes`, as a vt100 screen of the full size.
    fn outer(writes: &[String]) -> vt100::Parser {
        let mut parser = vt100::Parser::new(6, 40, 0);
        for write in writes {
            parser.process(write.as_bytes());
        }
        parser
    }

    fn row_text(parser: &vt100::Parser, row: u16) -> String {
        parser.screen().contents_between(row, 0, row, 40)
    }

    #[test]
    fn too_small_a_terminal_gets_no_line() {
        assert!(StatusLine::new("s", "t", 4, 80).is_none());
        assert_eq!(line().agent_rows(), 4);
    }

    #[test]
    fn a_rule_separates_the_line_from_the_agent() {
        let mut status = line();
        let screen = outer(&[status.setup(), status.filter_output("agent")]);
        assert!(row_text(&screen, 0).contains("deckhand"));
        assert_eq!(row_text(&screen, 1).trim_end(), "─".repeat(39));
        assert_eq!(row_text(&screen, 2), "agent");
    }

    #[test]
    fn the_line_survives_a_scroll_region_reset_and_scrolling() {
        let mut status = line();
        let mut writes = vec![status.setup()];
        writes.push(status.filter_output("\x1b[r"));
        for n in 0..20 {
            writes.push(status.filter_output(&format!("line {n}\r\n")));
        }
        let screen = outer(&writes);
        assert!(row_text(&screen, 0).contains("deckhand"));
        assert!(row_text(&screen, 1).starts_with('─'));
        assert!(screen.screen().contents().contains("line 19"));
    }

    #[test]
    fn clearing_the_screen_draws_the_line_again_and_keeps_the_cursor() {
        let mut status = line();
        let mut writes = vec![status.setup()];
        writes.push(status.filter_output("hello\x1b[2J\x1b[3;5Hx"));
        let screen = outer(&writes);
        assert!(row_text(&screen, 0).contains("deckhand"));
        assert!(row_text(&screen, 1).starts_with('─'));
        // The agent's row 3 is the terminal's row 5.
        assert_eq!(screen.screen().cursor_position(), (4, 5));
        assert!(row_text(&screen, 4).starts_with("    x"));
    }

    #[test]
    fn erasing_toward_the_line_draws_it_again() {
        for erase in ["\x1b[1J", "\x1b[2J", "\x1b[3J"] {
            let mut status = line();
            let mut writes = vec![status.setup()];
            writes.push(status.filter_output(&format!("\x1b[2;1H{erase}")));
            let screen = outer(&writes);
            assert!(row_text(&screen, 0).contains("deckhand"), "{erase:?}");
            assert!(row_text(&screen, 1).starts_with('─'), "{erase:?}");
        }
        // Erasing below the cursor never reaches it.
        assert!(!line().filter_output("\x1b[J").contains("deckhand"));
    }

    #[test]
    fn rows_are_clamped_and_moved_below_the_line() {
        let mut status = line();
        // Six rows: the line, the rule, then the agent's four.
        assert_eq!(status.filter_output("\x1b[H"), "\x1b[3;H");
        assert_eq!(status.filter_output("\x1b[999;3H"), "\x1b[6;3H");
        assert_eq!(status.filter_output("\x1b[2;3f"), "\x1b[4;3f");
        assert_eq!(status.filter_output("\x1b[4d"), "\x1b[6d");
        assert_eq!(status.filter_output("\x1b[99d"), "\x1b[6d");
        assert_eq!(status.filter_output("\x1b[r"), "\x1b[3;6r");
        assert_eq!(status.filter_output("\x1b[2;3r"), "\x1b[4;5r");
        assert_eq!(status.filter_output("\x1b[1;99r"), "\x1b[3;6r");
    }

    #[test]
    fn a_full_reset_restores_the_region_and_the_line() {
        let mut status = line();
        let mut writes = vec![status.setup()];
        writes.push(status.filter_output("\x1bc"));
        for n in 0..10 {
            writes.push(status.filter_output(&format!("row {n}\r\n")));
        }
        let screen = outer(&writes);
        assert!(row_text(&screen, 0).contains("deckhand"));
        assert!(row_text(&screen, 1).starts_with('─'));
    }

    #[test]
    fn redraws_wait_for_the_end_of_a_synchronized_update() {
        let mut status = line();
        let during = status.filter_output("\x1b[?2026h\x1b[2J");
        assert!(!during.contains("deckhand"));
        let after = status.filter_output("frame\x1b[?2026l");
        assert!(after.contains("deckhand"));
    }

    #[test]
    fn a_sequence_cut_between_chunks_is_carried_over() {
        let mut status = line();
        assert_eq!(status.filter_output("ab\x1b[99"), "ab");
        assert_eq!(status.filter_output(";1H"), "\x1b[6;1H");
    }

    #[test]
    fn other_sessions_waiting_are_counted() {
        let mut status = StatusLine::new("s", "fix-login", 6, 80).unwrap();
        let session = |id: &str, state: Option<AttentionState>| -> SessionRecord {
            let mut value = serde_json::json!({
                "id": id, "title": id, "program": "claude", "command": "claude", "cwd": "/r", "repoRoot": "/r",
                "status": "running", "createdAt": "x", "updatedAt": "x"
            });
            if let Some(state) = state {
                value["attention"] = serde_json::json!({"state": state, "event": "Stop", "at": "x"});
            }
            serde_json::from_value(value).unwrap()
        };
        let drawn = status.set_sessions(&[
            session("s", Some(AttentionState::Working)),
            session("a", Some(AttentionState::NeedsInput)),
            session("b", Some(AttentionState::ResponseEnded)),
            session("c", None),
        ]);
        assert!(drawn.contains("2 other sessions waiting"));
        assert!(drawn.contains("working"));
        let unchanged = status.session_updated(&session("c", None));
        assert!(unchanged.is_empty());
        let drawn = status.session_removed("a");
        assert!(drawn.contains("1 other session waiting"));
    }

    #[test]
    fn mouse_reports_on_the_line_are_dropped_and_the_rest_moved_up() {
        let mut status = line();
        assert_eq!(status.filter_input("\x1b[<0;3;1Mk"), "k");
        assert_eq!(status.filter_input("\x1b[<0;3;2Mk"), "k");
        assert_eq!(status.filter_input("\x1b[<64;3;4M"), "\x1b[<64;3;2M");
        assert_eq!(status.filter_input("\x1b[A"), "\x1b[A");
    }

    #[test]
    fn cursor_reports_the_agent_asked_for_are_moved_up() {
        let mut status = line();
        // Not asked for: left alone (Shift+F3 is the same bytes as a report of row 1, column 2).
        assert_eq!(status.filter_input("\x1b[1;2R"), "\x1b[1;2R");
        assert_eq!(status.filter_output("\x1b[6n"), "\x1b[6n");
        assert_eq!(status.filter_input("\x1b[4;7R"), "\x1b[2;7R");
        assert_eq!(status.filter_input("\x1b[4;7R"), "\x1b[4;7R");
    }
}
