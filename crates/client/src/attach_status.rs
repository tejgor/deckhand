//! Not in the TS: a Deckhand status line on the bottom row of an attached agent or pane, with a rule over it that
//! separates it from the program.
//!
//! The program's PTY gets [`HEIGHT`] rows less than the outer terminal and a scroll region keeps its output off the
//! bottom two, where Deckhand draws the rule and its own line (the session, how many other sessions wait on you, the
//! detach key). It is at the bottom for every attach: on the normal screen (Terminal, Dev, actions) a scroll region
//! starting at the top row still feeds the terminal's scrollback (most terminals, xterm.js among them, only keep lines
//! that scroll out of such a region), and the full-screen attaches keep it in the same place. The program's output
//! still goes straight to the terminal; on the way it is adjusted so it cannot reach those rows:
//! - scroll regions are clamped to the program's rows (a reset means all of them);
//! - absolute row moves (`CUP`, `HVP`, `VPA`) are clamped to the program's rows;
//! - after anything that erases or replaces those rows (erasing the display, a full reset, switching screens) they are
//!   drawn again, and a full reset or a screen switch also restores the scroll region (some terminals reset it there).
//!
//! Redrawing must leave the cursor and colours where the agent had them. The agent may be using the terminal's one
//! saved-cursor slot itself, so instead of saving the cursor the agent's output is mirrored into a small emulator
//! (`vt100`, in the agent's own coordinates) that knows the cursor position and drawing attributes to restore. Redraws
//! wait for the end of the agent's synchronized update, and are wrapped in one themselves.

use std::fmt::Write as _;

use indexmap::IndexMap;
use unicode_width::UnicodeWidthChar;

use deckhand_core::agent_signals::needs_attention;
use deckhand_core::display::{DUSK, Rgb, ThemeMode};
use deckhand_protocol::data::{AttentionState, SessionRecord, SessionStatus};

/// The rows Deckhand takes: the line, and the rule between it and the program.
const HEIGHT: u16 = 2;

/// Fewer outer rows than this leaves the agent too little room: no status line.
const MIN_ROWS: u16 = HEIGHT + 3;

/// The status line's SGR colours, for one [`ThemeMode`].
#[derive(Clone, Debug, PartialEq, Eq)]
struct Colours {
    /// The line's background and plain text.
    line: String,
    /// ` deckhand `, bold.
    brand: String,
    /// Separators and quiet text, not bold.
    quiet: String,
    title: String,
    /// The other sessions waiting, bold.
    waiting: String,
    /// The rule over the line (on the terminal's background).
    rule: String,
    working: String,
    needs_you: String,
    done: String,
    failed: String,
    idle: String,
}

impl Colours {
    fn of(theme: ThemeMode) -> Self {
        match theme {
            ThemeMode::Terminal => Colours {
                line: "\x1b[48;5;236m\x1b[38;5;250m".to_owned(),
                brand: "\x1b[1;38;5;212m".to_owned(),
                quiet: "\x1b[22;38;5;245m".to_owned(),
                title: "\x1b[38;5;252m".to_owned(),
                waiting: "\x1b[1;38;5;221m".to_owned(),
                rule: "\x1b[38;5;239m".to_owned(),
                working: "\x1b[38;5;117m".to_owned(),
                needs_you: "\x1b[38;5;221m".to_owned(),
                done: "\x1b[38;5;114m".to_owned(),
                failed: "\x1b[38;5;203m".to_owned(),
                idle: "\x1b[38;5;245m".to_owned(),
            },
            // The design system's chrome: a title bar in its own background, glyph colours as in the UI.
            ThemeMode::Dusk => Colours {
                line: format!("\x1b[{};{}m", sgr(48, DUSK.chrome), sgr(38, DUSK.muted)),
                brand: format!("\x1b[1;{}m", sgr(38, DUSK.magenta)),
                quiet: format!("\x1b[22;{}m", sgr(38, DUSK.dim)),
                title: format!("\x1b[{}m", sgr(38, DUSK.ink)),
                waiting: format!("\x1b[1;{}m", sgr(38, DUSK.yellow)),
                rule: format!("\x1b[{}m", sgr(38, DUSK.rule)),
                working: format!("\x1b[{}m", sgr(38, DUSK.green)),
                needs_you: format!("\x1b[{}m", sgr(38, DUSK.yellow)),
                done: format!("\x1b[{}m", sgr(38, DUSK.green)),
                failed: format!("\x1b[{}m", sgr(38, DUSK.red)),
                idle: format!("\x1b[{}m", sgr(38, DUSK.dim)),
            },
        }
    }

    fn state(&self, state: AttentionState) -> &str {
        match state {
            AttentionState::Working => &self.working,
            AttentionState::NeedsInput | AttentionState::Limited => &self.needs_you,
            AttentionState::ResponseEnded => &self.done,
            AttentionState::Failed => &self.failed,
            AttentionState::Unknown => &self.idle,
        }
    }
}

/// The SGR parameters of a 24-bit colour (`layer` 38: foreground, 48: background).
fn sgr(layer: u8, Rgb(r, g, b): Rgb) -> String {
    format!("{layer};2;{r};{g};{b}")
}

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
    /// The pane's name, shown after the title instead of the agent's state (`None`: the agent).
    pane: Option<String>,
    /// The outer terminal's size; the program gets `rows - HEIGHT`.
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
    sessions: IndexMap<String, SessionSummary>,
    colours: Colours,
}

impl StatusLine {
    /// A status line for an outer terminal of `rows` × `cols` in `theme`'s colours, or `None` when it is too small for
    /// one.
    pub fn new(session_id: &str, title: &str, rows: i64, cols: i64, theme: ThemeMode) -> Option<Self> {
        let (rows, cols) = (clamp_u16(rows), clamp_u16(cols));
        if rows < MIN_ROWS || cols == 0 {
            return None;
        }
        Some(StatusLine {
            session_id: session_id.to_owned(),
            title: title.to_owned(),
            pane: None,
            rows,
            cols,
            mirror: vt100::Parser::new(rows - HEIGHT, cols, 0),
            carry: String::new(),
            in_sync: false,
            redraw_due: false,
            sessions: IndexMap::new(),
            colours: Colours::of(theme),
        })
    }

    /// The line of a pane attach: `pane` (its name) after the title instead of the agent's state.
    #[must_use]
    pub fn on_pane(mut self, pane: impl Into<String>) -> Self {
        self.pane = Some(pane.into());
        self
    }

    /// The rows the program's PTY gets.
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

    /// Input on its way to the program: SGR mouse reports on the rule or the line are dropped.
    pub fn filter_input(&mut self, data: &str) -> String {
        let agent_rows = u32::from(self.rows - HEIGHT);
        let mut out = String::with_capacity(data.len());
        let mut rest = data;
        while let Some(at) = rest.find("\x1b[") {
            out.push_str(&rest[..at]);
            let sequence = &rest[at..];
            if let Some((len, row)) = sgr_mouse(sequence) {
                if row <= agent_rows {
                    out.push_str(&sequence[..len]);
                }
                rest = &sequence[len..];
                continue;
            }
            out.push_str("\x1b[");
            rest = &sequence[2..];
        }
        out.push_str(rest);
        out
    }

    /// Clears the rule and the line (written before handing the terminal back).
    pub fn teardown(&self) -> String {
        let (rule, line) = (self.rows - 1, self.rows);
        format!("\x1b[r\x1b[0m\x1b[{rule};1H\x1b[2K\x1b[{line};1H\x1b[2K")
    }

    /// The scroll region that keeps the program off the rule and the line: every row above them.
    fn region(&self) -> String {
        format!("\x1b[1;{}r", self.rows - HEIGHT)
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
            let mut switched = false;
            for mode in modes {
                match (mode, last) {
                    (2026, b'h') => self.in_sync = true,
                    (2026, b'l') => self.in_sync = false,
                    (47 | 1047 | 1049, b'h' | b'l') => switched = true,
                    _ => {}
                }
            }
            return if switched { Adjusted::Reset } else { Adjusted::Keep };
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
        // Within the agent's rows already: nothing to clamp.
        let in_rows = param(0).is_none_or(|first| first <= agent_rows);
        match last {
            // DECSTBM: never past the agent's rows (a reset means all of them).
            b'r' => {
                let region = format!(
                    "\x1b[{};{}r",
                    row(0),
                    param(1).unwrap_or(agent_rows).clamp(1, agent_rows)
                );
                Adjusted::Replace {
                    agent: region.clone(),
                    terminal: region,
                }
            }
            // CUP / HVP: the row clamped, the column kept.
            b'H' | b'f' if !in_rows => {
                let col = params
                    .get(1)
                    .copied()
                    .flatten()
                    .map(|col| col.to_string())
                    .unwrap_or_default();
                let clamped = format!("\x1b[{};{col}{}", row(0), last as char);
                Adjusted::Replace {
                    agent: clamped.clone(),
                    terminal: clamped,
                }
            }
            // VPA.
            b'd' if !in_rows => {
                let clamped = format!("\x1b[{}d", row(0));
                Adjusted::Replace {
                    agent: clamped.clone(),
                    terminal: clamped,
                }
            }
            // ED: 0 (cursor to end), 2 (all) and 3 (all and scrollback) reach the rule and the line; 1 (start to cursor)
            // never does.
            b'J' => match param(0).unwrap_or(0) {
                0 | 2 | 3 => Adjusted::Redraw,
                _ => Adjusted::Keep,
            },
            _ => Adjusted::Keep,
        }
    }

    /// The rule and the line under it, drawn on the bottom two rows, then the agent's cursor and drawing attributes put
    /// back.
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
        let colours = &self.colours;
        let _ = write!(out, "\x1b[{};1H\x1b[0m", self.rows);
        out.push_str(&colours.line);
        let mut line = Line::new(usize::from(self.cols));
        line.push(&colours.brand, " deckhand ");
        line.push(&colours.quiet, "│ ");
        line.push(&colours.title, &self.title);
        if let Some(pane) = &self.pane {
            line.push(&colours.quiet, " · ");
            line.push(&colours.title, pane);
        } else if let Some(state) = own {
            line.push(&colours.quiet, " · ");
            line.push(colours.state(state), state_label(state));
        }
        line.push(&colours.quiet, " │ ");
        if waiting > 0 {
            let plural = if waiting == 1 { "" } else { "s" };
            line.push(&colours.waiting, &format!("! {waiting} other session{plural} waiting"));
            line.push("\x1b[22m", "");
        } else {
            line.push(&colours.quiet, "nothing else waiting");
        }
        line.push(&colours.quiet, " │ Ctrl+] detach");
        out.push_str(&line.finish());
        let _ = write!(out, "\x1b[0m\x1b[{};1H", self.rows - 1);
        // Like the line, the rule leaves the last cell empty.
        let mut rule = Line::new(usize::from(self.cols));
        rule.push(&colours.rule, &"─".repeat(usize::from(self.cols - 1)));
        out.push_str(&rule.finish());
        out.push_str("\x1b[0m");
        // The mirror's coordinates are the terminal's: its formatter also restores a pending wrap.
        let screen = self.mirror.screen();
        out.push_str(&String::from_utf8_lossy(&screen.cursor_state_formatted()));
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
    /// Keep it, then draw the line again (it was erased).
    Redraw,
    /// A full reset or a screen switch: keep it, restore the scroll region, draw the line again.
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A 6 × 40 terminal: the program's four rows, the rule, the line.
    fn line() -> StatusLine {
        StatusLine::new("s", "fix-login", 6, 40, ThemeMode::Terminal).unwrap()
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

    /// The rule and the line are in place on the bottom two rows.
    fn holds_its_rows(parser: &vt100::Parser) -> bool {
        row_text(parser, 4).starts_with('─') && row_text(parser, 5).starts_with(" deckhand │ fix-login")
    }

    #[test]
    fn dusk_draws_the_line_in_the_design_colours() {
        let status = StatusLine::new("s", "fix-login", 6, 40, ThemeMode::Dusk).unwrap();
        let parser = outer(&[status.setup()]);
        let screen = parser.screen();
        assert!(holds_its_rows(&parser));
        let brand = screen.cell(5, 1).unwrap();
        assert_eq!(brand.fgcolor(), vt100::Color::Rgb(0xf7, 0xa8, 0xf0));
        assert_eq!(brand.bgcolor(), vt100::Color::Rgb(0x42, 0x49, 0x55));
        assert!(brand.bold());
        let title = screen.cell(5, 12).unwrap();
        assert_eq!(title.fgcolor(), vt100::Color::Rgb(0xf4, 0xf2, 0xec));
        assert!(!title.bold());
        assert_eq!(
            screen.cell(4, 0).unwrap().fgcolor(),
            vt100::Color::Rgb(0x59, 0x61, 0x6e)
        );
        // The terminal theme keeps its 256-colour line.
        let terminal = outer(&[line().setup()]);
        assert_eq!(terminal.screen().cell(5, 1).unwrap().fgcolor(), vt100::Color::Idx(212));
    }

    #[test]
    fn too_small_a_terminal_gets_no_line() {
        assert!(StatusLine::new("s", "t", 4, 80, ThemeMode::Terminal).is_none());
        assert_eq!(line().agent_rows(), 4);
    }

    #[test]
    fn a_rule_separates_the_program_from_the_line() {
        let mut status = line();
        let screen = outer(&[status.setup(), status.filter_output("agent")]);
        assert_eq!(row_text(&screen, 0), "agent");
        assert_eq!(row_text(&screen, 4).trim_end(), "─".repeat(39));
        assert!(holds_its_rows(&screen));
        assert!(status.setup().starts_with("\x1b[1;4r"));
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
        assert!(holds_its_rows(&screen));
        assert!(screen.screen().contents().contains("line 19"));
    }

    #[test]
    fn clearing_the_screen_draws_the_line_again_and_keeps_the_cursor() {
        let mut status = line();
        let mut writes = vec![status.setup()];
        writes.push(status.filter_output("hello\x1b[2J\x1b[3;5Hx"));
        let screen = outer(&writes);
        assert!(holds_its_rows(&screen));
        assert_eq!(screen.screen().cursor_position(), (2, 5));
        assert!(row_text(&screen, 2).starts_with("    x"));
    }

    #[test]
    fn erasing_toward_the_line_draws_it_again() {
        for erase in ["\x1b[J", "\x1b[2J", "\x1b[3J"] {
            let mut status = line();
            let mut writes = vec![status.setup()];
            writes.push(status.filter_output(&format!("\x1b[2;1H{erase}")));
            assert!(holds_its_rows(&outer(&writes)), "{erase:?}");
        }
        // Erasing above the cursor never reaches it.
        assert!(!line().filter_output("\x1b[1J").contains("deckhand"));
    }

    #[test]
    fn rows_are_clamped_to_the_program() {
        let mut status = line();
        assert_eq!(status.filter_output("\x1b[H"), "\x1b[H");
        assert_eq!(status.filter_output("\x1b[2;3H"), "\x1b[2;3H");
        assert_eq!(status.filter_output("\x1b[999;3H"), "\x1b[4;3H");
        assert_eq!(status.filter_output("\x1b[5;3f"), "\x1b[4;3f");
        assert_eq!(status.filter_output("\x1b[99d"), "\x1b[4d");
        assert_eq!(status.filter_output("\x1b[r"), "\x1b[1;4r");
        assert_eq!(status.filter_output("\x1b[2;3r"), "\x1b[2;3r");
        assert_eq!(status.filter_output("\x1b[1;99r"), "\x1b[1;4r");
    }

    #[test]
    fn a_full_reset_restores_the_region_and_the_line() {
        let mut status = line();
        let mut writes = vec![status.setup()];
        writes.push(status.filter_output("\x1bc"));
        for n in 0..10 {
            writes.push(status.filter_output(&format!("row {n}\r\n")));
        }
        assert!(holds_its_rows(&outer(&writes)));
    }

    #[test]
    fn switching_screens_restores_the_region_and_the_line() {
        let mut status = line();
        for switch in ["\x1b[?1049h", "\x1b[?1049l", "\x1b[?47h"] {
            let out = status.filter_output(switch);
            assert!(out.starts_with(&format!("{switch}\x1b[1;4r")), "{switch:?}: {out:?}");
            assert!(out.contains("deckhand"), "{switch:?}");
        }
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
        assert_eq!(status.filter_output(";1H"), "\x1b[4;1H");
    }

    #[test]
    fn other_sessions_waiting_are_counted() {
        let mut status = StatusLine::new("s", "fix-login", 6, 80, ThemeMode::Terminal).unwrap();
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
        // A pane shows its name instead of the agent's state.
        let mut pane = StatusLine::new("s", "fix-login", 6, 80, ThemeMode::Terminal)
            .unwrap()
            .on_pane("git");
        let drawn = pane.set_sessions(&[session("s", Some(AttentionState::Working))]);
        assert!(drawn.contains("fix-login") && drawn.contains("git"));
        assert!(!drawn.contains("working"));
    }

    #[test]
    fn mouse_reports_on_the_rule_or_the_line_are_dropped() {
        let mut status = line();
        assert_eq!(status.filter_input("\x1b[<64;3;4Mk"), "\x1b[<64;3;4Mk");
        assert_eq!(status.filter_input("\x1b[<0;3;5M\x1b[<0;3;6Mk"), "k");
        assert_eq!(status.filter_input("\x1b[A"), "\x1b[A");
        // Cursor position reports are the program's own coordinates: left alone.
        assert_eq!(status.filter_output("\x1b[6n"), "\x1b[6n");
        assert_eq!(status.filter_input("\x1b[4;7R"), "\x1b[4;7R");
    }

    #[test]
    fn teardown_clears_the_rule_and_the_line() {
        assert_eq!(line().teardown(), "\x1b[r\x1b[0m\x1b[5;1H\x1b[2K\x1b[6;1H\x1b[2K");
    }
}
