//! The `HeadlessTerminal` implementation, on alacritty_terminal.
//!
//! The TS preview reads `@xterm/headless` 5.5, and the H0c goldens (fixtures/term/*.golden.json) pin exactly what that
//! emulator shows. alacritty_terminal's own `Term` disagrees with it on things the goldens observe: it measures widths
//! with Unicode 15 (xterm uses its UnicodeV6 table, so 🎉 is one cell), it keeps a wrap flag where xterm parks the
//! cursor at `x == cols`, and it reflows and trims differently on resize. So the screen model here is a port of the
//! xterm.js core (EscapeSequenceParser, InputHandler, Buffer, BufferLine, BufferReflow, CircularList, UnicodeV6) for
//! everything with a visible effect; alacritty_terminal supplies the event and dimension types ([`EventSink`],
//! [`TermDimensions`]). Replies xterm would send (DA, DSR, DECRQM, DECRQSS) are swallowed, as TerminalPreview ignores
//! `onData`. Where xterm throws (a missing line, a list index out of range) the rest of that parse chunk is dropped,
//! as xterm's WriteBuffer does.

use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};
use std::rc::Rc;
use std::sync::OnceLock;

use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::grid::Dimensions;

use crate::headless::{
    BufferKind, CellAttrs, CellFlags, CellInfo, Color, CursorPos, HeadlessTerminal, MouseTrackingMode, TerminalOptions,
    XtermModes,
};
use crate::utf8::Utf8Decoder;

/// TS: @xterm/headless Terminal events deckhand observes (onTitleChange); also swallows replies (onData).
#[derive(Debug, Clone, Default)]
pub struct EventSink {
    /// The last title set by OSC 0/2 (`None` until one is set or after a reset).
    title: Rc<RefCell<Option<String>>>,
}

impl EventSink {
    /// The last title received, `None` if none was set.
    pub fn title(&self) -> Option<String> {
        self.title.try_borrow().ok().and_then(|title| title.clone())
    }
}

impl EventListener for EventSink {
    /// TS: Terminal.onTitleChange
    fn send_event(&self, event: Event) {
        let next = match event {
            Event::Title(title) => Some(title),
            Event::ResetTitle => None,
            _ => return,
        };
        if let Ok(mut title) = self.title.try_borrow_mut() {
            *title = next;
        }
    }
}

/// Grid size handed to alacritty's `Term::new`/`Term::resize`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TermDimensions {
    pub columns: usize,
    pub screen_lines: usize,
}

impl Dimensions for TermDimensions {
    fn total_lines(&self) -> usize {
        self.screen_lines
    }

    fn screen_lines(&self) -> usize {
        self.screen_lines
    }

    fn columns(&self) -> usize {
        self.columns
    }
}

/// TS: @xterm/headless Terminal, implemented on alacritty_terminal 0.25.
pub struct AlacrittyTerminal {
    core: Core,
    /// node-pty's decoding for callers that hand over raw bytes; text written through `write_str` passes unchanged.
    decoder: Utf8Decoder,
    events: EventSink,
    options: TerminalOptions,
}

impl AlacrittyTerminal {
    /// The options the terminal was created with.
    pub fn options(&self) -> TerminalOptions {
        self.options
    }

    fn active(&self) -> &Buffer {
        self.core.active()
    }
}

impl HeadlessTerminal for AlacrittyTerminal {
    /// TS: new Terminal({cols, rows, scrollback, allowProposedApi: true})
    fn new(options: TerminalOptions) -> Self {
        let events = EventSink::default();
        AlacrittyTerminal {
            core: Core::new(
                i64::from(options.cols),
                i64::from(options.rows),
                options.scrollback,
                events.clone(),
            ),
            decoder: Utf8Decoder::new(),
            events,
            options,
        }
    }

    /// TS: Terminal.write
    fn write(&mut self, data: &[u8]) {
        let text = self.decoder.write(data);
        self.core.write(&text);
    }

    /// TS: Terminal.resize
    fn resize(&mut self, cols: u16, rows: u16) {
        self.core.resize(i64::from(cols), i64::from(rows));
    }

    /// TS: Terminal.cols
    fn cols(&self) -> u16 {
        to_u16(self.core.cols)
    }

    /// TS: Terminal.rows
    fn rows(&self) -> u16 {
        to_u16(self.core.rows)
    }

    /// TS: Terminal.buffer.active.type
    fn buffer_kind(&self) -> BufferKind {
        if self.core.alt_active {
            BufferKind::Alternate
        } else {
            BufferKind::Normal
        }
    }

    /// TS: Terminal.buffer.active.baseY
    fn base_y(&self) -> usize {
        to_usize(self.active().ybase)
    }

    /// TS: Terminal.buffer.active.length
    fn length(&self) -> usize {
        self.active().lines.length
    }

    /// TS: IBuffer.getLine(y)?.translateToString(trimRight)
    fn line_text(&self, y: usize, trim_right: bool) -> Option<String> {
        let line = self.active().lines.get(to_i64(y))?;
        let text = line.borrow().translate_to_string(trim_right);
        Some(text)
    }

    /// TS: IBuffer.getLine(y)?.isWrapped
    fn line_is_wrapped(&self, y: usize) -> bool {
        self.active()
            .lines
            .get(to_i64(y))
            .is_some_and(|line| line.borrow().is_wrapped)
    }

    /// TS: IBuffer.getLine(y)?.getCell(x)
    fn cell(&self, y: usize, x: u16) -> Option<CellInfo> {
        let line = self.active().lines.get(to_i64(y))?;
        let line = line.borrow();
        let x = i64::from(x);
        if x >= line.len() {
            return None;
        }
        let mut cell = CellData::default();
        line.load_cell(x, &mut cell);
        Some(cell.info())
    }

    /// TS: IBuffer.cursorX / cursorY
    fn cursor(&self) -> CursorPos {
        let buffer = self.active();
        CursorPos {
            x: to_u16(buffer.x),
            y: to_u16(buffer.y),
        }
    }

    /// TS: Terminal.modes
    fn modes(&self) -> XtermModes {
        let core = &self.core;
        XtermModes {
            application_cursor_keys_mode: core.dec.application_cursor_keys,
            application_keypad_mode: core.dec.application_keypad,
            bracketed_paste_mode: core.dec.bracketed_paste_mode,
            insert_mode: core.insert_mode,
            mouse_tracking_mode: core.mouse_protocol,
            origin_mode: core.dec.origin,
            reverse_wraparound_mode: core.dec.reverse_wraparound,
            send_focus_mode: core.dec.send_focus,
            wraparound_mode: core.dec.wraparound,
        }
    }

    /// TS: Terminal.onTitleChange (last value)
    fn title(&self) -> Option<String> {
        self.events.title()
    }

    /// TS: CoreService.isCursorHidden, negated
    fn cursor_visible(&self) -> bool {
        !self.core.dec.cursor_hidden
    }

    /// TS: IBuffer.viewportY (`ydisp`, which xterm moves separately from `ybase` on resize and reflow)
    fn viewport_y(&self) -> usize {
        to_usize(self.active().ydisp)
    }
}

fn to_u16(value: i64) -> u16 {
    u16::try_from(value.max(0)).unwrap_or(u16::MAX)
}

fn to_usize(value: i64) -> usize {
    usize::try_from(value).unwrap_or(0)
}

fn to_i64(value: usize) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

// ── Cell encoding (common/buffer/Constants.ts) ───────────────────────────────────────────────────────────────────

const CODEPOINT_MASK: u32 = 0x1F_FFFF;
const IS_COMBINED_MASK: u32 = 0x20_0000;
const HAS_CONTENT_MASK: u32 = 0x3F_FFFF;
const WIDTH_MASK: u32 = 0xC0_0000;
const WIDTH_SHIFT: u32 = 22;

const CM_MASK: u32 = 0x300_0000;
const CM_P16: u32 = 0x100_0000;
const CM_P256: u32 = 0x200_0000;
const CM_RGB: u32 = 0x300_0000;
const RGB_MASK: u32 = 0xFF_FFFF;
const PCOLOR_MASK: u32 = 0xFF;

const FG_INVERSE: u32 = 0x400_0000;
const FG_BOLD: u32 = 0x800_0000;
const FG_UNDERLINE: u32 = 0x1000_0000;
const FG_BLINK: u32 = 0x2000_0000;
const FG_INVISIBLE: u32 = 0x4000_0000;
const FG_STRIKETHROUGH: u32 = 0x8000_0000;

const BG_ITALIC: u32 = 0x400_0000;
const BG_DIM: u32 = 0x800_0000;
const BG_HAS_EXTENDED: u32 = 0x1000_0000;
const BG_PROTECTED: u32 = 0x2000_0000;
const BG_OVERLINE: u32 = 0x4000_0000;

const EXT_UNDERLINE_STYLE: u32 = 0x1C00_0000;

const UNDERLINE_NONE: u32 = 0;
const UNDERLINE_SINGLE: u32 = 1;
const UNDERLINE_DOUBLE: u32 = 2;
const UNDERLINE_DASHED: u32 = 5;

const TAB_STOP_WIDTH: i64 = 8;
const MINIMUM_COLS: i64 = 2;
const MINIMUM_ROWS: i64 = 1;
/// InputHandler MAX_PARSEBUFFER_LENGTH: longer writes are parsed in chunks of this many UTF-16 units.
const MAX_PARSEBUFFER_LENGTH: usize = 131_072;
/// OscParser PAYLOAD_LIMIT (UTF-16 units).
const PAYLOAD_LIMIT: usize = 10_000_000;
/// REP repeats at most this many code points. xterm allocates `text.length * count` up front and gives up (throws)
/// on absurd counts; this bounds the work instead of attempting gigabytes.
const MAX_REPEAT_CODEPOINTS: i64 = 1 << 22;

fn string_from_code_point(code_point: u32) -> String {
    char::from_u32(code_point)
        .unwrap_or(char::REPLACEMENT_CHARACTER)
        .to_string()
}

/// TS: ExtendedAttrs (underline style/colour and the OSC 8 link id)
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct ExtendedAttrs {
    ext: u32,
    url_id: u32,
}

impl ExtendedAttrs {
    fn underline_style(&self) -> u32 {
        // Linked text always reports the dashed style.
        if self.url_id != 0 {
            return UNDERLINE_DASHED;
        }
        (self.ext & EXT_UNDERLINE_STYLE) >> 26
    }

    fn set_underline_style(&mut self, style: u32) {
        self.ext &= !EXT_UNDERLINE_STYLE;
        self.ext |= (style << 26) & EXT_UNDERLINE_STYLE;
    }

    fn underline_color(&self) -> u32 {
        self.ext & (CM_MASK | RGB_MASK)
    }

    fn set_underline_color(&mut self, color: u32) {
        self.ext &= !(CM_MASK | RGB_MASK);
        self.ext |= color & (CM_MASK | RGB_MASK);
    }

    fn is_empty(&self) -> bool {
        self.underline_style() == UNDERLINE_NONE && self.url_id == 0
    }
}

/// TS: AttributeData
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Attr {
    fg: u32,
    bg: u32,
    extended: ExtendedAttrs,
}

impl Attr {
    fn update_extended(&mut self) {
        if self.extended.is_empty() {
            self.bg &= !BG_HAS_EXTENDED;
        } else {
            self.bg |= BG_HAS_EXTENDED;
        }
    }
}

/// TS: CellData
#[derive(Debug, Clone, Default)]
struct CellData {
    content: u32,
    attr: Attr,
    combined: String,
}

impl CellData {
    /// TS: Buffer.getNullCell(attr) — an empty one-cell-wide cell in `attr`.
    fn null(attr: Attr) -> Self {
        CellData {
            content: 1 << WIDTH_SHIFT,
            attr,
            combined: String::new(),
        }
    }

    fn chars(&self) -> String {
        if self.content & IS_COMBINED_MASK != 0 {
            return self.combined.clone();
        }
        if self.content & CODEPOINT_MASK != 0 {
            return string_from_code_point(self.content & CODEPOINT_MASK);
        }
        String::new()
    }

    /// The cell as xterm's IBufferCell reports it (the encoding of the goldens in fixtures/term).
    fn info(&self) -> CellInfo {
        let Attr { fg, bg, extended } = self.attr;
        let mut flags = CellFlags::empty();
        let underline = if bg & BG_HAS_EXTENDED != 0 && extended.underline_style() != UNDERLINE_NONE {
            true
        } else {
            fg & FG_UNDERLINE != 0
        };
        flags.set(CellFlags::BOLD, fg & FG_BOLD != 0);
        flags.set(CellFlags::DIM, bg & BG_DIM != 0);
        flags.set(CellFlags::ITALIC, bg & BG_ITALIC != 0);
        flags.set(CellFlags::UNDERLINE, underline);
        flags.set(CellFlags::BLINK, fg & FG_BLINK != 0);
        flags.set(CellFlags::INVERSE, fg & FG_INVERSE != 0);
        flags.set(CellFlags::INVISIBLE, fg & FG_INVISIBLE != 0);
        flags.set(CellFlags::STRIKETHROUGH, fg & FG_STRIKETHROUGH != 0);
        flags.set(CellFlags::OVERLINE, bg & BG_OVERLINE != 0);
        CellInfo {
            chars: self.chars(),
            width: u8::try_from(self.content >> WIDTH_SHIFT).unwrap_or(0),
            attrs: CellAttrs {
                fg: color(fg),
                bg: color(bg),
                flags,
            },
        }
    }
}

/// TS: getFgColorMode/getFgColor as the goldens in fixtures/term encode it
fn color(value: u32) -> Color {
    let low = (value & PCOLOR_MASK) as u8;
    match value & CM_MASK {
        CM_P16 => Color::P16(low),
        CM_P256 => Color::P256(low),
        CM_RGB => Color::Rgb((value >> 16) as u8, (value >> 8) as u8, low),
        _ => Color::Default,
    }
}

// ── BufferLine ───────────────────────────────────────────────────────────────────────────────────────────────────

/// TS: BufferLine. Out-of-range reads give 0 and writes are dropped, like the Uint32Array behind it.
#[derive(Debug, Clone, Default)]
struct BufferLine {
    /// Per cell: content, fg, bg.
    data: Vec<[u32; 3]>,
    combined: HashMap<i64, String>,
    extended: HashMap<i64, ExtendedAttrs>,
    is_wrapped: bool,
}

type LineRef = Rc<RefCell<BufferLine>>;

fn line_ref(line: BufferLine) -> LineRef {
    Rc::new(RefCell::new(line))
}

impl BufferLine {
    fn new(cols: i64, fill: &CellData, is_wrapped: bool) -> Self {
        let count = to_usize(cols);
        let mut line = BufferLine {
            data: vec![[0; 3]; count],
            combined: HashMap::new(),
            extended: HashMap::new(),
            is_wrapped,
        };
        for index in 0..cols {
            line.set_cell(index, fill);
        }
        line
    }

    fn len(&self) -> i64 {
        to_i64(self.data.len())
    }

    fn word(&self, index: i64, field: usize) -> u32 {
        usize::try_from(index)
            .ok()
            .and_then(|index| self.data.get(index))
            .and_then(|cell| cell.get(field))
            .copied()
            .unwrap_or(0)
    }

    fn slot_mut(&mut self, index: i64) -> Option<&mut [u32; 3]> {
        usize::try_from(index).ok().and_then(|index| self.data.get_mut(index))
    }

    fn content(&self, index: i64) -> u32 {
        self.word(index, 0)
    }

    fn get_fg(&self, index: i64) -> u32 {
        self.word(index, 1)
    }

    fn get_bg(&self, index: i64) -> u32 {
        self.word(index, 2)
    }

    fn get_width(&self, index: i64) -> u32 {
        self.content(index) >> WIDTH_SHIFT
    }

    fn has_width(&self, index: i64) -> u32 {
        self.content(index) & WIDTH_MASK
    }

    fn has_content(&self, index: i64) -> u32 {
        self.content(index) & HAS_CONTENT_MASK
    }

    fn is_protected(&self, index: i64) -> bool {
        self.get_bg(index) & BG_PROTECTED != 0
    }

    fn get_string(&self, index: i64) -> String {
        let content = self.content(index);
        if content & IS_COMBINED_MASK != 0 {
            return self.combined.get(&index).cloned().unwrap_or_default();
        }
        if content & CODEPOINT_MASK != 0 {
            return string_from_code_point(content & CODEPOINT_MASK);
        }
        String::new()
    }

    fn load_cell(&self, index: i64, cell: &mut CellData) {
        cell.content = self.content(index);
        cell.attr.fg = self.get_fg(index);
        cell.attr.bg = self.get_bg(index);
        if cell.content & IS_COMBINED_MASK != 0 {
            cell.combined = self.combined.get(&index).cloned().unwrap_or_default();
        }
        if cell.attr.bg & BG_HAS_EXTENDED != 0
            && let Some(extended) = self.extended.get(&index)
        {
            cell.attr.extended = *extended;
        }
    }

    fn set_cell(&mut self, index: i64, cell: &CellData) {
        if cell.content & IS_COMBINED_MASK != 0 {
            self.combined.insert(index, cell.combined.clone());
        }
        if cell.attr.bg & BG_HAS_EXTENDED != 0 {
            self.extended.insert(index, cell.attr.extended);
        }
        if let Some(slot) = self.slot_mut(index) {
            *slot = [cell.content, cell.attr.fg, cell.attr.bg];
        }
    }

    fn set_cell_from_codepoint(&mut self, index: i64, code_point: u32, width: u32, attr: &Attr) {
        if attr.bg & BG_HAS_EXTENDED != 0 {
            self.extended.insert(index, attr.extended);
        }
        if let Some(slot) = self.slot_mut(index) {
            *slot = [code_point | (width << WIDTH_SHIFT), attr.fg, attr.bg];
        }
    }

    fn add_codepoint_to_cell(&mut self, index: i64, code_point: u32, width: u32) {
        let mut content = self.content(index);
        if content & IS_COMBINED_MASK != 0 {
            self.combined
                .entry(index)
                .or_default()
                .push_str(&string_from_code_point(code_point));
        } else if content & CODEPOINT_MASK != 0 {
            let mut text = string_from_code_point(content & CODEPOINT_MASK);
            text.push_str(&string_from_code_point(code_point));
            self.combined.insert(index, text);
            content &= !CODEPOINT_MASK;
            content |= IS_COMBINED_MASK;
        } else {
            content = code_point | (1 << WIDTH_SHIFT);
        }
        if width != 0 {
            content &= !WIDTH_MASK;
            content |= width << WIDTH_SHIFT;
        }
        if let Some(slot) = self.slot_mut(index) {
            slot[0] = content;
        }
    }

    fn insert_cells(&mut self, pos: i64, n: i64, fill: &CellData) {
        let length = self.len();
        if length == 0 {
            return;
        }
        let pos = pos % length;
        if pos != 0 && self.get_width(pos - 1) == 2 {
            self.set_cell_from_codepoint(pos - 1, 0, 1, &fill.attr);
        }
        if n < length - pos {
            let mut cell = CellData::default();
            for i in (0..length - pos - n).rev() {
                self.load_cell(pos + i, &mut cell);
                self.set_cell(pos + n + i, &cell);
            }
            for i in 0..n {
                self.set_cell(pos + i, fill);
            }
        } else {
            for i in pos..length {
                self.set_cell(i, fill);
            }
        }
        if self.get_width(length - 1) == 2 {
            self.set_cell_from_codepoint(length - 1, 0, 1, &fill.attr);
        }
    }

    fn delete_cells(&mut self, pos: i64, n: i64, fill: &CellData) {
        let length = self.len();
        if length == 0 {
            return;
        }
        let pos = pos % length;
        if n < length - pos {
            let mut cell = CellData::default();
            for i in 0..length - pos - n {
                self.load_cell(pos + n + i, &mut cell);
                self.set_cell(pos + i, &cell);
            }
            for i in length - n..length {
                self.set_cell(i, fill);
            }
        } else {
            for i in pos..length {
                self.set_cell(i, fill);
            }
        }
        if pos != 0 && self.get_width(pos - 1) == 2 {
            self.set_cell_from_codepoint(pos - 1, 0, 1, &fill.attr);
        }
        if self.get_width(pos) == 0 && self.has_content(pos) == 0 {
            self.set_cell_from_codepoint(pos, 0, 1, &fill.attr);
        }
    }

    fn replace_cells(&mut self, mut start: i64, end: i64, fill: &CellData, respect_protect: bool) {
        let length = self.len();
        if respect_protect {
            if start != 0 && self.get_width(start - 1) == 2 && !self.is_protected(start - 1) {
                self.set_cell_from_codepoint(start - 1, 0, 1, &fill.attr);
            }
            if end < length && self.get_width(end - 1) == 2 && !self.is_protected(end) {
                self.set_cell_from_codepoint(end, 0, 1, &fill.attr);
            }
            while start < end && start < length {
                if !self.is_protected(start) {
                    self.set_cell(start, fill);
                }
                start += 1;
            }
            return;
        }
        if start != 0 && self.get_width(start - 1) == 2 {
            self.set_cell_from_codepoint(start - 1, 0, 1, &fill.attr);
        }
        if end < length && self.get_width(end - 1) == 2 {
            self.set_cell_from_codepoint(end, 0, 1, &fill.attr);
        }
        while start < end && start < length {
            self.set_cell(start, fill);
            start += 1;
        }
    }

    fn resize(&mut self, cols: i64, fill: &CellData) {
        let length = self.len();
        if cols == length {
            return;
        }
        if cols > length {
            self.data.resize(to_usize(cols), [0; 3]);
            for index in length..cols {
                self.set_cell(index, fill);
            }
        } else {
            self.data.truncate(to_usize(cols));
            self.combined.retain(|key, _| *key < cols);
            self.extended.retain(|key, _| *key < cols);
        }
    }

    fn fill(&mut self, fill: &CellData, respect_protect: bool) {
        if respect_protect {
            for index in 0..self.len() {
                if !self.is_protected(index) {
                    self.set_cell(index, fill);
                }
            }
            return;
        }
        self.combined.clear();
        self.extended.clear();
        for index in 0..self.len() {
            self.set_cell(index, fill);
        }
    }

    fn copy_from(&mut self, line: &BufferLine) {
        self.clone_from(line);
    }

    fn get_trimmed_length(&self) -> i64 {
        for index in (0..self.len()).rev() {
            let content = self.content(index);
            if content & HAS_CONTENT_MASK != 0 {
                return index + i64::from(content >> WIDTH_SHIFT);
            }
        }
        0
    }

    /// TS: copyCellsFrom with a different source line.
    fn copy_cells_from(&mut self, src: &BufferLine, src_col: i64, dest_col: i64, length: i64, reverse: bool) {
        let copy = |line: &mut BufferLine, cell: i64| {
            let values = [
                src.word(src_col + cell, 0),
                src.word(src_col + cell, 1),
                src.word(src_col + cell, 2),
            ];
            if let Some(slot) = line.slot_mut(dest_col + cell) {
                *slot = values;
            }
            if values[2] & BG_HAS_EXTENDED != 0
                && let Some(extended) = src.extended.get(&(src_col + cell))
            {
                line.extended.insert(dest_col + cell, *extended);
            }
        };
        if reverse {
            for cell in (0..length).rev() {
                copy(self, cell);
            }
        } else {
            for cell in 0..length {
                copy(self, cell);
            }
        }
        for (key, text) in &src.combined {
            if *key >= src_col {
                self.combined.insert(key - src_col + dest_col, text.clone());
            }
        }
    }

    /// TS: copyCellsFrom with `src === this`: every step reads what earlier steps wrote, as in JS.
    fn copy_cells_within(&mut self, src_col: i64, dest_col: i64, length: i64, reverse: bool) {
        let copy = |line: &mut BufferLine, cell: i64| {
            let values = [
                line.word(src_col + cell, 0),
                line.word(src_col + cell, 1),
                line.word(src_col + cell, 2),
            ];
            if let Some(slot) = line.slot_mut(dest_col + cell) {
                *slot = values;
            }
            if values[2] & BG_HAS_EXTENDED != 0
                && let Some(extended) = line.extended.get(&(src_col + cell)).copied()
            {
                line.extended.insert(dest_col + cell, extended);
            }
        };
        if reverse {
            for cell in (0..length).rev() {
                copy(self, cell);
            }
        } else {
            for cell in 0..length {
                copy(self, cell);
            }
        }
        let mut keys: Vec<i64> = self.combined.keys().copied().collect();
        keys.sort_unstable();
        for key in keys {
            if key >= src_col
                && let Some(text) = self.combined.get(&key).cloned()
            {
                self.combined.insert(key - src_col + dest_col, text);
            }
        }
    }

    fn translate_to_string(&self, trim_right: bool) -> String {
        let mut start = 0;
        let mut end = self.len();
        if trim_right {
            end = end.min(self.get_trimmed_length());
        }
        let mut result = String::new();
        while start < end {
            let content = self.content(start);
            let code_point = content & CODEPOINT_MASK;
            if content & IS_COMBINED_MASK != 0 {
                if let Some(text) = self.combined.get(&start) {
                    result.push_str(text);
                }
            } else if code_point != 0 {
                result.push(char::from_u32(code_point).unwrap_or(char::REPLACEMENT_CHARACTER));
            } else {
                result.push(' ');
            }
            start += i64::from((content >> WIDTH_SHIFT).max(1));
        }
        result
    }
}

/// TS: copyCellsFrom on two lines that may be the same object.
fn copy_cells(dest: &LineRef, src: &LineRef, src_col: i64, dest_col: i64, length: i64, reverse: bool) {
    if Rc::ptr_eq(dest, src) {
        dest.borrow_mut().copy_cells_within(src_col, dest_col, length, reverse);
    } else {
        let src = src.borrow();
        dest.borrow_mut()
            .copy_cells_from(&src, src_col, dest_col, length, reverse);
    }
}

// ── CircularList ─────────────────────────────────────────────────────────────────────────────────────────────────

/// TS: CircularList<IBufferLine>. Like the JS original, `get` does not check `length`: a slot past the end still
/// returns whatever line it last held.
#[derive(Debug, Clone)]
struct CircularList {
    array: Vec<Option<LineRef>>,
    start: usize,
    length: usize,
    max_length: usize,
}

/// A JS exception inside a handler: the rest of the parse chunk is dropped.
#[derive(Debug, Clone, Copy)]
struct Abort;

type Step = Result<(), Abort>;

impl CircularList {
    fn new(max_length: usize) -> Self {
        CircularList {
            array: Vec::new(),
            start: 0,
            length: 0,
            max_length: max_length.max(1),
        }
    }

    fn slot(&self, index: i64) -> Option<usize> {
        let absolute = to_i64(self.start).checked_add(index)?;
        usize::try_from(absolute)
            .ok()
            .map(|absolute| absolute % self.max_length)
    }

    fn get(&self, index: i64) -> Option<LineRef> {
        self.slot(index)
            .and_then(|slot| self.array.get(slot))
            .cloned()
            .flatten()
    }

    fn put(&mut self, slot: usize, value: Option<LineRef>) {
        if slot >= self.array.len() {
            if value.is_none() {
                return;
            }
            self.array.resize(slot + 1, None);
        }
        if let Some(entry) = self.array.get_mut(slot) {
            *entry = value;
        }
    }

    fn set(&mut self, index: i64, value: Option<LineRef>) {
        if let Some(slot) = self.slot(index) {
            self.put(slot, value);
        }
    }

    fn len(&self) -> i64 {
        to_i64(self.length)
    }

    fn set_max_length(&mut self, max_length: usize) {
        let max_length = max_length.max(1);
        if self.max_length == max_length {
            return;
        }
        let kept = max_length.min(self.length);
        let array = (0..kept).map(|index| self.get(to_i64(index))).collect();
        self.array = array;
        self.max_length = max_length;
        self.start = 0;
    }

    /// TS: `lines.length = n` — growing clears raw (not cyclic) slots, exactly as the JS setter does.
    fn set_length(&mut self, length: usize) {
        if length > self.length {
            for slot in self.length..length {
                self.put(slot, None);
            }
        }
        self.length = length;
    }

    fn push(&mut self, value: LineRef) {
        self.set(self.len(), Some(value));
        if self.length == self.max_length {
            self.start = (self.start + 1) % self.max_length;
        } else {
            self.length += 1;
        }
    }

    fn recycle(&mut self) -> Option<LineRef> {
        if self.length != self.max_length {
            return None;
        }
        self.start = (self.start + 1) % self.max_length;
        self.get(self.len() - 1)
    }

    fn is_full(&self) -> bool {
        self.length == self.max_length
    }

    fn pop(&mut self) -> Option<LineRef> {
        let line = self.get(self.len() - 1);
        self.length = self.length.saturating_sub(1);
        line
    }

    fn splice(&mut self, start: i64, delete_count: usize, items: Vec<LineRef>) {
        let delete = to_i64(delete_count);
        if delete_count > 0 {
            let mut index = start;
            while index < self.len() - delete {
                let line = self.get(index + delete);
                self.set(index, line);
                index += 1;
            }
            self.length = self.length.saturating_sub(delete_count);
        }
        let count = to_i64(items.len());
        let mut index = self.len() - 1;
        while index >= start {
            let line = self.get(index);
            self.set(index + count, line);
            index -= 1;
        }
        let added = items.len();
        for (offset, item) in (0..).zip(items) {
            self.set(start + offset, Some(item));
        }
        if self.length + added > self.max_length {
            let trim = self.length + added - self.max_length;
            self.start += trim;
            self.length = self.max_length;
        } else {
            self.length += added;
        }
    }

    fn trim_start(&mut self, count: usize) {
        let count = count.min(self.length);
        self.start += count;
        self.length -= count;
    }

    fn shift_elements(&mut self, start: i64, count: i64, offset: i64) -> Step {
        if count <= 0 {
            return Ok(());
        }
        if start < 0 || start >= self.len() || start + offset < 0 {
            return Err(Abort);
        }
        if offset > 0 {
            for i in (0..count).rev() {
                let line = self.get(start + i);
                self.set(start + i + offset, line);
            }
            let expand = start + count + offset - self.len();
            if expand > 0 {
                self.length += to_usize(expand);
                while self.length > self.max_length {
                    self.length -= 1;
                    self.start += 1;
                }
            }
        } else {
            for i in 0..count {
                let line = self.get(start + i);
                self.set(start + i + offset, line);
            }
        }
        Ok(())
    }
}

// ── Charsets (common/data/Charsets.ts) ───────────────────────────────────────────────────────────────────────────

/// A designated character set, by its final byte; `None` stands for the default (US ASCII, `B`).
type Charset = Option<u8>;

/// TS: CHARSETS keys (`B` maps to undefined, i.e. the default).
const CHARSET_FLAGS: &[u8] = b"0AB4C5RQKYE6ZH7=";

fn charset_map(flag: u8, code: u32) -> Option<char> {
    let ch = char::from_u32(code)?;
    let mapped = match flag {
        b'0' => match ch {
            '`' => '\u{25c6}',
            'a' => '\u{2592}',
            'b' => '\u{2409}',
            'c' => '\u{240c}',
            'd' => '\u{240d}',
            'e' => '\u{240a}',
            'f' => '\u{00b0}',
            'g' => '\u{00b1}',
            'h' => '\u{2424}',
            'i' => '\u{240b}',
            'j' => '\u{2518}',
            'k' => '\u{2510}',
            'l' => '\u{250c}',
            'm' => '\u{2514}',
            'n' => '\u{253c}',
            'o' => '\u{23ba}',
            'p' => '\u{23bb}',
            'q' => '\u{2500}',
            'r' => '\u{23bc}',
            's' => '\u{23bd}',
            't' => '\u{251c}',
            'u' => '\u{2524}',
            'v' => '\u{2534}',
            'w' => '\u{252c}',
            'x' => '\u{2502}',
            'y' => '\u{2264}',
            'z' => '\u{2265}',
            '{' => '\u{03c0}',
            '|' => '\u{2260}',
            '}' => '\u{00a3}',
            '~' => '\u{00b7}',
            _ => return None,
        },
        b'A' => match ch {
            '#' => '£',
            _ => return None,
        },
        // `[` maps to "ij"; xterm keeps only its first code unit.
        b'4' => match ch {
            '#' => '£',
            '@' => '¾',
            '[' => 'i',
            '\\' => '½',
            ']' => '|',
            '{' => '¨',
            '|' => 'f',
            '}' => '¼',
            '~' => '´',
            _ => return None,
        },
        b'C' | b'5' => match ch {
            '[' => 'Ä',
            '\\' => 'Ö',
            ']' => 'Å',
            '^' => 'Ü',
            '`' => 'é',
            '{' => 'ä',
            '|' => 'ö',
            '}' => 'å',
            '~' => 'ü',
            _ => return None,
        },
        b'R' => match ch {
            '#' => '£',
            '@' => 'à',
            '[' => '°',
            '\\' => 'ç',
            ']' => '§',
            '{' => 'é',
            '|' => 'ù',
            '}' => 'è',
            '~' => '¨',
            _ => return None,
        },
        b'Q' => match ch {
            '@' => 'à',
            '[' => 'â',
            '\\' => 'ç',
            ']' => 'ê',
            '^' => 'î',
            '`' => 'ô',
            '{' => 'é',
            '|' => 'ù',
            '}' => 'è',
            '~' => 'û',
            _ => return None,
        },
        b'K' => match ch {
            '@' => '§',
            '[' => 'Ä',
            '\\' => 'Ö',
            ']' => 'Ü',
            '{' => 'ä',
            '|' => 'ö',
            '}' => 'ü',
            '~' => 'ß',
            _ => return None,
        },
        b'Y' => match ch {
            '#' => '£',
            '@' => '§',
            '[' => '°',
            '\\' => 'ç',
            ']' => 'é',
            '`' => 'ù',
            '{' => 'à',
            '|' => 'ò',
            '}' => 'è',
            '~' => 'ì',
            _ => return None,
        },
        b'E' | b'6' => match ch {
            '@' => 'Ä',
            '[' => 'Æ',
            '\\' => 'Ø',
            ']' => 'Å',
            '^' => 'Ü',
            '`' => 'ä',
            '{' => 'æ',
            '|' => 'ø',
            '}' => 'å',
            '~' => 'ü',
            _ => return None,
        },
        b'Z' => match ch {
            '#' => '£',
            '@' => '§',
            '[' => '¡',
            '\\' => 'Ñ',
            ']' => '¿',
            '{' => '°',
            '|' => 'ñ',
            '}' => 'ç',
            _ => return None,
        },
        b'H' | b'7' => match ch {
            '@' => 'É',
            '[' => 'Ä',
            '\\' => 'Ö',
            ']' => 'Å',
            '^' => 'Ü',
            '`' => 'é',
            '{' => 'ä',
            '|' => 'ö',
            '}' => 'å',
            '~' => 'ü',
            _ => return None,
        },
        b'=' => match ch {
            '#' => 'ù',
            '@' => 'à',
            '[' => 'é',
            '\\' => 'ç',
            ']' => 'ê',
            '^' => 'î',
            '_' => 'è',
            '`' => 'ô',
            '{' => 'ä',
            '|' => 'ö',
            '}' => 'ü',
            '~' => 'û',
            _ => return None,
        },
        _ => return None,
    };
    Some(mapped)
}

/// TS: CHARSETS[flag] — `B` (and anything unknown) is the default set.
fn charset_for(flag: u8) -> Charset {
    (flag != b'B' && CHARSET_FLAGS.contains(&flag)).then_some(flag)
}

// ── UnicodeV6 (common/input/UnicodeV6.ts) ────────────────────────────────────────────────────────────────────────

const BMP_COMBINING: &[(u32, u32)] = &[
    (0x0300, 0x036F),
    (0x0483, 0x0486),
    (0x0488, 0x0489),
    (0x0591, 0x05BD),
    (0x05BF, 0x05BF),
    (0x05C1, 0x05C2),
    (0x05C4, 0x05C5),
    (0x05C7, 0x05C7),
    (0x0600, 0x0603),
    (0x0610, 0x0615),
    (0x064B, 0x065E),
    (0x0670, 0x0670),
    (0x06D6, 0x06E4),
    (0x06E7, 0x06E8),
    (0x06EA, 0x06ED),
    (0x070F, 0x070F),
    (0x0711, 0x0711),
    (0x0730, 0x074A),
    (0x07A6, 0x07B0),
    (0x07EB, 0x07F3),
    (0x0901, 0x0902),
    (0x093C, 0x093C),
    (0x0941, 0x0948),
    (0x094D, 0x094D),
    (0x0951, 0x0954),
    (0x0962, 0x0963),
    (0x0981, 0x0981),
    (0x09BC, 0x09BC),
    (0x09C1, 0x09C4),
    (0x09CD, 0x09CD),
    (0x09E2, 0x09E3),
    (0x0A01, 0x0A02),
    (0x0A3C, 0x0A3C),
    (0x0A41, 0x0A42),
    (0x0A47, 0x0A48),
    (0x0A4B, 0x0A4D),
    (0x0A70, 0x0A71),
    (0x0A81, 0x0A82),
    (0x0ABC, 0x0ABC),
    (0x0AC1, 0x0AC5),
    (0x0AC7, 0x0AC8),
    (0x0ACD, 0x0ACD),
    (0x0AE2, 0x0AE3),
    (0x0B01, 0x0B01),
    (0x0B3C, 0x0B3C),
    (0x0B3F, 0x0B3F),
    (0x0B41, 0x0B43),
    (0x0B4D, 0x0B4D),
    (0x0B56, 0x0B56),
    (0x0B82, 0x0B82),
    (0x0BC0, 0x0BC0),
    (0x0BCD, 0x0BCD),
    (0x0C3E, 0x0C40),
    (0x0C46, 0x0C48),
    (0x0C4A, 0x0C4D),
    (0x0C55, 0x0C56),
    (0x0CBC, 0x0CBC),
    (0x0CBF, 0x0CBF),
    (0x0CC6, 0x0CC6),
    (0x0CCC, 0x0CCD),
    (0x0CE2, 0x0CE3),
    (0x0D41, 0x0D43),
    (0x0D4D, 0x0D4D),
    (0x0DCA, 0x0DCA),
    (0x0DD2, 0x0DD4),
    (0x0DD6, 0x0DD6),
    (0x0E31, 0x0E31),
    (0x0E34, 0x0E3A),
    (0x0E47, 0x0E4E),
    (0x0EB1, 0x0EB1),
    (0x0EB4, 0x0EB9),
    (0x0EBB, 0x0EBC),
    (0x0EC8, 0x0ECD),
    (0x0F18, 0x0F19),
    (0x0F35, 0x0F35),
    (0x0F37, 0x0F37),
    (0x0F39, 0x0F39),
    (0x0F71, 0x0F7E),
    (0x0F80, 0x0F84),
    (0x0F86, 0x0F87),
    (0x0F90, 0x0F97),
    (0x0F99, 0x0FBC),
    (0x0FC6, 0x0FC6),
    (0x102D, 0x1030),
    (0x1032, 0x1032),
    (0x1036, 0x1037),
    (0x1039, 0x1039),
    (0x1058, 0x1059),
    (0x1160, 0x11FF),
    (0x135F, 0x135F),
    (0x1712, 0x1714),
    (0x1732, 0x1734),
    (0x1752, 0x1753),
    (0x1772, 0x1773),
    (0x17B4, 0x17B5),
    (0x17B7, 0x17BD),
    (0x17C6, 0x17C6),
    (0x17C9, 0x17D3),
    (0x17DD, 0x17DD),
    (0x180B, 0x180D),
    (0x18A9, 0x18A9),
    (0x1920, 0x1922),
    (0x1927, 0x1928),
    (0x1932, 0x1932),
    (0x1939, 0x193B),
    (0x1A17, 0x1A18),
    (0x1B00, 0x1B03),
    (0x1B34, 0x1B34),
    (0x1B36, 0x1B3A),
    (0x1B3C, 0x1B3C),
    (0x1B42, 0x1B42),
    (0x1B6B, 0x1B73),
    (0x1DC0, 0x1DCA),
    (0x1DFE, 0x1DFF),
    (0x200B, 0x200F),
    (0x202A, 0x202E),
    (0x2060, 0x2063),
    (0x206A, 0x206F),
    (0x20D0, 0x20EF),
    (0x302A, 0x302F),
    (0x3099, 0x309A),
    (0xA806, 0xA806),
    (0xA80B, 0xA80B),
    (0xA825, 0xA826),
    (0xFB1E, 0xFB1E),
    (0xFE00, 0xFE0F),
    (0xFE20, 0xFE23),
    (0xFEFF, 0xFEFF),
    (0xFFF9, 0xFFFB),
];

const HIGH_COMBINING: &[(u32, u32)] = &[
    (0x10A01, 0x10A03),
    (0x10A05, 0x10A06),
    (0x10A0C, 0x10A0F),
    (0x10A38, 0x10A3A),
    (0x10A3F, 0x10A3F),
    (0x1D167, 0x1D169),
    (0x1D173, 0x1D182),
    (0x1D185, 0x1D18B),
    (0x1D1AA, 0x1D1AD),
    (0x1D242, 0x1D244),
    (0xE0001, 0xE0001),
    (0xE0020, 0xE007F),
    (0xE0100, 0xE01EF),
];

fn bmp_width_table() -> &'static [u8] {
    static TABLE: OnceLock<Vec<u8>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut table = vec![1u8; 65536];
        let mut fill = |value: u8, start: usize, end: usize| {
            if let Some(range) = table.get_mut(start..end) {
                range.fill(value);
            }
        };
        fill(0, 0, 32);
        fill(0, 0x7f, 0xa0);
        fill(2, 0x1100, 0x1160);
        fill(2, 0x2329, 0x232b);
        fill(2, 0x2e80, 0xa4d0);
        fill(1, 0x303f, 0x3040);
        fill(2, 0xac00, 0xd7a4);
        fill(2, 0xf900, 0xfb00);
        fill(2, 0xfe10, 0xfe1a);
        fill(2, 0xfe30, 0xfe70);
        fill(2, 0xff00, 0xff61);
        fill(2, 0xffe0, 0xffe7);
        for &(start, end) in BMP_COMBINING {
            fill(0, start as usize, end as usize + 1);
        }
        table
    })
}

/// TS: UnicodeV6.wcwidth
fn wcwidth(code: u32) -> u32 {
    if code < 32 {
        return 0;
    }
    if code < 127 {
        return 1;
    }
    if code < 65536 {
        return bmp_width_table().get(code as usize).copied().map_or(1, u32::from);
    }
    if HIGH_COMBINING.iter().any(|&(start, end)| (start..=end).contains(&code)) {
        return 0;
    }
    if (0x20000..=0x2fffd).contains(&code) || (0x30000..=0x3fffd).contains(&code) {
        return 2;
    }
    1
}

fn extract_width(properties: u32) -> u32 {
    (properties >> 1) & 0x3
}

fn extract_should_join(properties: u32) -> bool {
    properties & 1 != 0
}

/// TS: UnicodeV6.charProperties — zero-width code points join the preceding cell when it has a width.
fn char_properties(code: u32, preceding: u32) -> u32 {
    let mut width = wcwidth(code);
    let mut should_join = width == 0 && preceding != 0;
    if should_join {
        let old_width = extract_width(preceding);
        if old_width == 0 {
            should_join = false;
        } else if old_width > width {
            width = old_width;
        }
    }
    ((width & 3) << 1) | u32::from(should_join)
}

// ── Parser (common/parser/*.ts) ──────────────────────────────────────────────────────────────────────────────────

mod state {
    pub const GROUND: u8 = 0;
    pub const ESCAPE: u8 = 1;
    pub const ESCAPE_INTERMEDIATE: u8 = 2;
    pub const CSI_ENTRY: u8 = 3;
    pub const CSI_PARAM: u8 = 4;
    pub const CSI_INTERMEDIATE: u8 = 5;
    pub const CSI_IGNORE: u8 = 6;
    pub const SOS_PM_APC_STRING: u8 = 7;
    pub const OSC_STRING: u8 = 8;
    pub const DCS_ENTRY: u8 = 9;
    pub const DCS_PARAM: u8 = 10;
    pub const DCS_IGNORE: u8 = 11;
    pub const DCS_INTERMEDIATE: u8 = 12;
    pub const DCS_PASSTHROUGH: u8 = 13;
}

mod action {
    pub const IGNORE: u8 = 0;
    pub const ERROR: u8 = 1;
    pub const PRINT: u8 = 2;
    pub const EXECUTE: u8 = 3;
    pub const OSC_START: u8 = 4;
    pub const OSC_PUT: u8 = 5;
    pub const OSC_END: u8 = 6;
    pub const CSI_DISPATCH: u8 = 7;
    pub const PARAM: u8 = 8;
    pub const COLLECT: u8 = 9;
    pub const ESC_DISPATCH: u8 = 10;
    pub const CLEAR: u8 = 11;
    pub const DCS_HOOK: u8 = 12;
    pub const DCS_PUT: u8 = 13;
    pub const DCS_UNHOOK: u8 = 14;
}

/// Pseudo code for every printable code point from U+00A0 up.
const NON_ASCII_PRINTABLE: u32 = 0xA0;
const TABLE_SIZE: usize = (state::DCS_PASSTHROUGH as usize + 1) << 8;

/// TS: VT500_TRANSITION_TABLE — `action << 4 | next state`, indexed by `state << 8 | code`.
fn transition_table() -> &'static [u8] {
    static TABLE: OnceLock<Vec<u8>> = OnceLock::new();
    TABLE.get_or_init(|| {
        use action::*;
        use state::*;
        let mut table = vec![(ERROR << 4) | GROUND; TABLE_SIZE];
        let mut add = |codes: &[u32], state: u8, action: u8, next: u8| {
            for &code in codes {
                if let Some(entry) = table.get_mut((usize::from(state) << 8) | code as usize) {
                    *entry = (action << 4) | next;
                }
            }
        };
        let r = |start: u32, end: u32| (start..end).collect::<Vec<u32>>();
        let printables = r(0x20, 0x7f);
        let mut executables = r(0x00, 0x18);
        executables.push(0x19);
        executables.extend(r(0x1c, 0x20));

        add(&printables, GROUND, PRINT, GROUND);
        for state in GROUND..=DCS_PASSTHROUGH {
            add(&[0x18, 0x1a, 0x99, 0x9a], state, EXECUTE, GROUND);
            add(&r(0x80, 0x90), state, EXECUTE, GROUND);
            add(&r(0x90, 0x98), state, EXECUTE, GROUND);
            add(&[0x9c], state, IGNORE, GROUND);
            add(&[0x1b], state, CLEAR, ESCAPE);
            add(&[0x9d], state, OSC_START, OSC_STRING);
            add(&[0x98, 0x9e, 0x9f], state, IGNORE, SOS_PM_APC_STRING);
            add(&[0x9b], state, CLEAR, CSI_ENTRY);
            add(&[0x90], state, CLEAR, DCS_ENTRY);
        }
        add(&executables, GROUND, EXECUTE, GROUND);
        add(&executables, ESCAPE, EXECUTE, ESCAPE);
        add(&[0x7f], ESCAPE, IGNORE, ESCAPE);
        add(&executables, OSC_STRING, IGNORE, OSC_STRING);
        add(&executables, CSI_ENTRY, EXECUTE, CSI_ENTRY);
        add(&[0x7f], CSI_ENTRY, IGNORE, CSI_ENTRY);
        add(&executables, CSI_PARAM, EXECUTE, CSI_PARAM);
        add(&[0x7f], CSI_PARAM, IGNORE, CSI_PARAM);
        add(&executables, CSI_IGNORE, EXECUTE, CSI_IGNORE);
        add(&executables, CSI_INTERMEDIATE, EXECUTE, CSI_INTERMEDIATE);
        add(&[0x7f], CSI_INTERMEDIATE, IGNORE, CSI_INTERMEDIATE);
        add(&executables, ESCAPE_INTERMEDIATE, EXECUTE, ESCAPE_INTERMEDIATE);
        add(&[0x7f], ESCAPE_INTERMEDIATE, IGNORE, ESCAPE_INTERMEDIATE);
        // osc
        add(&[0x5d], ESCAPE, OSC_START, OSC_STRING);
        add(&printables, OSC_STRING, OSC_PUT, OSC_STRING);
        add(&[0x7f], OSC_STRING, OSC_PUT, OSC_STRING);
        add(&[0x9c, 0x1b, 0x18, 0x1a, 0x07], OSC_STRING, OSC_END, GROUND);
        add(&r(0x1c, 0x20), OSC_STRING, IGNORE, OSC_STRING);
        // sos/pm/apc does nothing
        add(&[0x58, 0x5e, 0x5f], ESCAPE, IGNORE, SOS_PM_APC_STRING);
        add(&printables, SOS_PM_APC_STRING, IGNORE, SOS_PM_APC_STRING);
        add(&executables, SOS_PM_APC_STRING, IGNORE, SOS_PM_APC_STRING);
        add(&[0x9c], SOS_PM_APC_STRING, IGNORE, GROUND);
        add(&[0x7f], SOS_PM_APC_STRING, IGNORE, SOS_PM_APC_STRING);
        // csi entries
        add(&[0x5b], ESCAPE, CLEAR, CSI_ENTRY);
        add(&r(0x40, 0x7f), CSI_ENTRY, CSI_DISPATCH, GROUND);
        add(&r(0x30, 0x3c), CSI_ENTRY, PARAM, CSI_PARAM);
        add(&[0x3c, 0x3d, 0x3e, 0x3f], CSI_ENTRY, COLLECT, CSI_PARAM);
        add(&r(0x30, 0x3c), CSI_PARAM, PARAM, CSI_PARAM);
        add(&r(0x40, 0x7f), CSI_PARAM, CSI_DISPATCH, GROUND);
        add(&[0x3c, 0x3d, 0x3e, 0x3f], CSI_PARAM, IGNORE, CSI_IGNORE);
        add(&r(0x20, 0x40), CSI_IGNORE, IGNORE, CSI_IGNORE);
        add(&[0x7f], CSI_IGNORE, IGNORE, CSI_IGNORE);
        add(&r(0x40, 0x7f), CSI_IGNORE, IGNORE, GROUND);
        add(&r(0x20, 0x30), CSI_ENTRY, COLLECT, CSI_INTERMEDIATE);
        add(&r(0x20, 0x30), CSI_INTERMEDIATE, COLLECT, CSI_INTERMEDIATE);
        add(&r(0x30, 0x40), CSI_INTERMEDIATE, IGNORE, CSI_IGNORE);
        add(&r(0x40, 0x7f), CSI_INTERMEDIATE, CSI_DISPATCH, GROUND);
        add(&r(0x20, 0x30), CSI_PARAM, COLLECT, CSI_INTERMEDIATE);
        // esc_intermediate
        add(&r(0x20, 0x30), ESCAPE, COLLECT, ESCAPE_INTERMEDIATE);
        add(&r(0x20, 0x30), ESCAPE_INTERMEDIATE, COLLECT, ESCAPE_INTERMEDIATE);
        add(&r(0x30, 0x7f), ESCAPE_INTERMEDIATE, ESC_DISPATCH, GROUND);
        add(&r(0x30, 0x50), ESCAPE, ESC_DISPATCH, GROUND);
        add(&r(0x51, 0x58), ESCAPE, ESC_DISPATCH, GROUND);
        add(&[0x59, 0x5a, 0x5c], ESCAPE, ESC_DISPATCH, GROUND);
        add(&r(0x60, 0x7f), ESCAPE, ESC_DISPATCH, GROUND);
        // dcs entry
        add(&[0x50], ESCAPE, CLEAR, DCS_ENTRY);
        add(&executables, DCS_ENTRY, IGNORE, DCS_ENTRY);
        add(&[0x7f], DCS_ENTRY, IGNORE, DCS_ENTRY);
        add(&r(0x1c, 0x20), DCS_ENTRY, IGNORE, DCS_ENTRY);
        add(&r(0x20, 0x30), DCS_ENTRY, COLLECT, DCS_INTERMEDIATE);
        add(&r(0x30, 0x3c), DCS_ENTRY, PARAM, DCS_PARAM);
        add(&[0x3c, 0x3d, 0x3e, 0x3f], DCS_ENTRY, COLLECT, DCS_PARAM);
        add(&executables, DCS_IGNORE, IGNORE, DCS_IGNORE);
        add(&r(0x20, 0x80), DCS_IGNORE, IGNORE, DCS_IGNORE);
        add(&r(0x1c, 0x20), DCS_IGNORE, IGNORE, DCS_IGNORE);
        add(&executables, DCS_PARAM, IGNORE, DCS_PARAM);
        add(&[0x7f], DCS_PARAM, IGNORE, DCS_PARAM);
        add(&r(0x1c, 0x20), DCS_PARAM, IGNORE, DCS_PARAM);
        add(&r(0x30, 0x3c), DCS_PARAM, PARAM, DCS_PARAM);
        add(&[0x3c, 0x3d, 0x3e, 0x3f], DCS_PARAM, IGNORE, DCS_IGNORE);
        add(&r(0x20, 0x30), DCS_PARAM, COLLECT, DCS_INTERMEDIATE);
        add(&executables, DCS_INTERMEDIATE, IGNORE, DCS_INTERMEDIATE);
        add(&[0x7f], DCS_INTERMEDIATE, IGNORE, DCS_INTERMEDIATE);
        add(&r(0x1c, 0x20), DCS_INTERMEDIATE, IGNORE, DCS_INTERMEDIATE);
        add(&r(0x20, 0x30), DCS_INTERMEDIATE, COLLECT, DCS_INTERMEDIATE);
        add(&r(0x30, 0x40), DCS_INTERMEDIATE, IGNORE, DCS_IGNORE);
        add(&r(0x40, 0x7f), DCS_INTERMEDIATE, DCS_HOOK, DCS_PASSTHROUGH);
        add(&r(0x40, 0x7f), DCS_PARAM, DCS_HOOK, DCS_PASSTHROUGH);
        add(&r(0x40, 0x7f), DCS_ENTRY, DCS_HOOK, DCS_PASSTHROUGH);
        add(&executables, DCS_PASSTHROUGH, DCS_PUT, DCS_PASSTHROUGH);
        add(&printables, DCS_PASSTHROUGH, DCS_PUT, DCS_PASSTHROUGH);
        add(&[0x7f], DCS_PASSTHROUGH, IGNORE, DCS_PASSTHROUGH);
        add(&[0x1b, 0x9c, 0x18, 0x1a], DCS_PASSTHROUGH, DCS_UNHOOK, GROUND);
        // special handling of unicode chars
        add(&[NON_ASCII_PRINTABLE], GROUND, PRINT, GROUND);
        add(&[NON_ASCII_PRINTABLE], OSC_STRING, OSC_PUT, OSC_STRING);
        add(&[NON_ASCII_PRINTABLE], CSI_IGNORE, IGNORE, CSI_IGNORE);
        add(&[NON_ASCII_PRINTABLE], DCS_IGNORE, IGNORE, DCS_IGNORE);
        add(&[NON_ASCII_PRINTABLE], DCS_PASSTHROUGH, DCS_PUT, DCS_PASSTHROUGH);
        table
    })
}

const MAX_PARAMS: usize = 32;
const MAX_SUB_PARAMS: usize = 32;
const MAX_PARAM_VALUE: i64 = 0x7FFF_FFFF;

/// TS: Params (ZDM: empty parameters are 0, empty sub parameters -1). Reads past `length` see stale values, as the
/// Int32Array behind it does.
#[derive(Debug, Clone)]
struct Params {
    params: [i32; MAX_PARAMS],
    length: usize,
    sub_params: [i32; MAX_SUB_PARAMS],
    sub_params_length: usize,
    sub_params_idx: [u16; MAX_PARAMS],
    reject_digits: bool,
    reject_sub_digits: bool,
    digit_is_sub: bool,
}

impl Default for Params {
    fn default() -> Self {
        Params {
            params: [0; MAX_PARAMS],
            length: 0,
            sub_params: [0; MAX_SUB_PARAMS],
            sub_params_length: 0,
            sub_params_idx: [0; MAX_PARAMS],
            reject_digits: false,
            reject_sub_digits: false,
            digit_is_sub: false,
        }
    }
}

impl Params {
    fn reset(&mut self) {
        self.length = 0;
        self.sub_params_length = 0;
        self.reject_digits = false;
        self.reject_sub_digits = false;
        self.digit_is_sub = false;
    }

    fn add_param(&mut self, value: i32) {
        self.digit_is_sub = false;
        if self.length >= MAX_PARAMS {
            self.reject_digits = true;
            return;
        }
        let idx = (self.sub_params_length << 8 | self.sub_params_length) as u16;
        if let Some(slot) = self.sub_params_idx.get_mut(self.length) {
            *slot = idx;
        }
        if let Some(slot) = self.params.get_mut(self.length) {
            *slot = value;
        }
        self.length += 1;
    }

    fn add_sub_param(&mut self, value: i32) {
        self.digit_is_sub = true;
        if self.length == 0 {
            return;
        }
        if self.reject_digits || self.sub_params_length >= MAX_SUB_PARAMS {
            self.reject_sub_digits = true;
            return;
        }
        if let Some(slot) = self.sub_params.get_mut(self.sub_params_length) {
            *slot = value;
        }
        self.sub_params_length += 1;
        if let Some(slot) = self.sub_params_idx.get_mut(self.length - 1) {
            *slot = slot.wrapping_add(1);
        }
    }

    fn add_digit(&mut self, value: i32) {
        let length = if self.digit_is_sub {
            self.sub_params_length
        } else {
            self.length
        };
        if self.reject_digits || length == 0 || (self.digit_is_sub && self.reject_sub_digits) {
            return;
        }
        let store = if self.digit_is_sub {
            &mut self.sub_params[..]
        } else {
            &mut self.params[..]
        };
        if let Some(cur) = store.get_mut(length - 1) {
            *cur = if *cur == -1 {
                value
            } else {
                (i64::from(*cur) * 10 + i64::from(value)).min(MAX_PARAM_VALUE) as i32
            };
        }
    }

    /// TS: `params.params[index]`
    fn param(&self, index: usize) -> i64 {
        self.params.get(index).copied().map_or(0, i64::from)
    }

    /// TS: `params.params[index] || fallback`
    fn param_or(&self, index: usize, fallback: i64) -> i64 {
        match self.param(index) {
            0 => fallback,
            value => value,
        }
    }

    fn sub_range(&self, index: usize) -> (usize, usize) {
        let idx = self.sub_params_idx.get(index).copied().unwrap_or(0);
        (usize::from(idx >> 8), usize::from(idx & 0xFF))
    }

    fn has_sub_params(&self, index: usize) -> bool {
        let (start, end) = self.sub_range(index);
        end > start
    }

    fn get_sub_params(&self, index: usize) -> &[i32] {
        let (start, end) = self.sub_range(index);
        self.sub_params.get(start..end).unwrap_or(&[])
    }
}

mod osc_state {
    pub const START: u8 = 0;
    pub const ID: u8 = 1;
    pub const PAYLOAD: u8 = 2;
    pub const ABORT: u8 = 3;
}

/// OSC identifiers InputHandler registers handlers for (the others go to the no-op fallback).
const OSC_HANDLED: &[i64] = &[0, 1, 2, 4, 8, 10, 11, 12, 104, 110, 111, 112];

/// TS: OscParser + OscHandler (one handler per identifier).
#[derive(Debug, Clone)]
struct OscParser {
    state: u8,
    id: i64,
    /// A handler is registered for `id`.
    active: bool,
    data: String,
    data_utf16: usize,
    hit_limit: bool,
}

impl Default for OscParser {
    fn default() -> Self {
        OscParser {
            state: osc_state::START,
            id: -1,
            active: false,
            data: String::new(),
            data_utf16: 0,
            hit_limit: false,
        }
    }
}

impl OscParser {
    fn reset(&mut self) {
        if self.state == osc_state::PAYLOAD && self.active {
            self.clear_data();
        }
        self.active = false;
        self.id = -1;
        self.state = osc_state::START;
    }

    fn clear_data(&mut self) {
        self.data.clear();
        self.data_utf16 = 0;
        self.hit_limit = false;
    }

    fn handler_start(&mut self) {
        self.active = OSC_HANDLED.contains(&self.id);
        if self.active {
            self.clear_data();
        }
    }

    fn start(&mut self) {
        self.reset();
        self.state = osc_state::ID;
    }

    fn put(&mut self, data: &[u32], mut start: usize, end: usize) {
        if self.state == osc_state::ABORT {
            return;
        }
        if self.state == osc_state::ID {
            while start < end {
                let code = data.get(start).copied().unwrap_or(0);
                start += 1;
                if code == 0x3b {
                    self.state = osc_state::PAYLOAD;
                    self.handler_start();
                    break;
                }
                if !(0x30..=0x39).contains(&code) {
                    self.state = osc_state::ABORT;
                    return;
                }
                if self.id == -1 {
                    self.id = 0;
                }
                self.id = self.id.saturating_mul(10).saturating_add(i64::from(code) - 48);
            }
        }
        if self.state == osc_state::PAYLOAD && end > start && self.active && !self.hit_limit {
            for &code in data.get(start..end).unwrap_or(&[]) {
                let ch = char::from_u32(code).unwrap_or(char::REPLACEMENT_CHARACTER);
                self.data.push(ch);
                self.data_utf16 += ch.len_utf16();
            }
            if self.data_utf16 > PAYLOAD_LIMIT {
                self.data.clear();
                self.data_utf16 = 0;
                self.hit_limit = true;
            }
        }
    }

    /// Ends the sequence; returns the identifier and payload to dispatch, if any.
    fn end(&mut self, success: bool) -> Option<(i64, String)> {
        if self.state == osc_state::START {
            return None;
        }
        let mut dispatch = None;
        if self.state != osc_state::ABORT {
            if self.state == osc_state::ID {
                self.handler_start();
            }
            if self.active {
                if !self.hit_limit && success {
                    dispatch = Some((self.id, std::mem::take(&mut self.data)));
                }
                self.clear_data();
            }
        }
        self.active = false;
        self.id = -1;
        self.state = osc_state::START;
        dispatch
    }
}

#[derive(Debug, Clone, Default)]
struct Parser {
    state: u8,
    params: Params,
    collect: u32,
    preceding_join_state: u32,
    osc: OscParser,
}

impl Parser {
    fn new() -> Self {
        let mut parser = Parser::default();
        parser.params.add_param(0);
        parser
    }

    fn clear(&mut self) {
        self.params.reset();
        self.params.add_param(0);
        self.collect = 0;
    }

    fn reset(&mut self) {
        self.state = state::GROUND;
        self.osc.reset();
        self.clear();
        self.preceding_join_state = 0;
    }
}

// ── Buffer (common/buffer/Buffer.ts, BufferReflow.ts) ───────────────────────────────────────────────────────────

/// TS: Buffer (one of the normal/alternate pair).
#[derive(Debug, Clone)]
struct Buffer {
    lines: CircularList,
    ydisp: i64,
    ybase: i64,
    y: i64,
    x: i64,
    scroll_bottom: i64,
    scroll_top: i64,
    tabs: BTreeSet<i64>,
    saved_y: i64,
    saved_x: i64,
    saved_attr: Attr,
    saved_charset: Charset,
    has_scrollback: bool,
    scrollback: usize,
    cols: i64,
    rows: i64,
}

fn default_null_cell() -> CellData {
    CellData::null(Attr::default())
}

impl Buffer {
    fn new(has_scrollback: bool, cols: i64, rows: i64, scrollback: usize) -> Self {
        let mut buffer = Buffer {
            lines: CircularList::new(0),
            ydisp: 0,
            ybase: 0,
            y: 0,
            x: 0,
            scroll_bottom: rows - 1,
            scroll_top: 0,
            tabs: BTreeSet::new(),
            saved_y: 0,
            saved_x: 0,
            saved_attr: Attr::default(),
            saved_charset: None,
            has_scrollback,
            scrollback,
            cols,
            rows,
        };
        buffer.lines = CircularList::new(buffer.correct_buffer_length(rows));
        buffer.setup_tab_stops(None);
        buffer
    }

    fn correct_buffer_length(&self, rows: i64) -> usize {
        let rows = to_usize(rows);
        if !self.has_scrollback {
            return rows;
        }
        rows.saturating_add(self.scrollback).min(u32::MAX as usize)
    }

    fn line(&self, index: i64) -> Option<LineRef> {
        self.lines.get(index)
    }

    fn fill_viewport_rows(&mut self, cols: i64, attr: Attr) {
        if self.lines.length == 0 {
            for _ in 0..self.rows {
                self.lines
                    .push(line_ref(BufferLine::new(cols, &CellData::null(attr), false)));
            }
        }
    }

    fn clear(&mut self) {
        self.ydisp = 0;
        self.ybase = 0;
        self.y = 0;
        self.x = 0;
        self.lines = CircularList::new(self.correct_buffer_length(self.rows));
        self.scroll_top = 0;
        self.scroll_bottom = self.rows - 1;
        self.setup_tab_stops(None);
    }

    fn setup_tab_stops(&mut self, from: Option<i64>) {
        let mut index = match from {
            Some(index) => {
                if self.tabs.contains(&index) {
                    index
                } else {
                    self.prev_stop(index)
                }
            }
            None => {
                self.tabs.clear();
                0
            }
        };
        while index < self.cols {
            self.tabs.insert(index);
            index += TAB_STOP_WIDTH;
        }
    }

    fn prev_stop(&self, mut x: i64) -> i64 {
        loop {
            x -= 1;
            if self.tabs.contains(&x) || x <= 0 {
                break;
            }
        }
        if x >= self.cols { self.cols - 1 } else { x.max(0) }
    }

    fn next_stop(&self, mut x: i64) -> i64 {
        loop {
            x += 1;
            if self.tabs.contains(&x) || x >= self.cols {
                break;
            }
        }
        if x >= self.cols { self.cols - 1 } else { x.max(0) }
    }

    /// TS: Buffer.resize — `new_cols` is also BufferService.cols, which blank lines take their width from.
    fn resize(&mut self, new_cols: i64, new_rows: i64) {
        let null_cell = default_null_cell();
        let new_max_length = self.correct_buffer_length(new_rows);
        if new_max_length > self.lines.max_length {
            self.lines.set_max_length(new_max_length);
        }
        if self.lines.length > 0 {
            if self.cols < new_cols {
                for index in 0..self.lines.len() {
                    if let Some(line) = self.line(index) {
                        line.borrow_mut().resize(new_cols, &null_cell);
                    }
                }
            }
            let mut add_to_y = 0;
            if self.rows < new_rows {
                for _ in self.rows..new_rows {
                    if self.lines.len() < new_rows + self.ybase {
                        if self.ybase > 0 && self.lines.len() <= self.ybase + self.y + add_to_y + 1 {
                            // Room above and no blank lines below the cursor: scroll up.
                            self.ybase -= 1;
                            add_to_y += 1;
                            if self.ydisp > 0 {
                                self.ydisp -= 1;
                            }
                        } else {
                            self.lines.push(line_ref(BufferLine::new(new_cols, &null_cell, false)));
                        }
                    }
                }
            } else {
                for _ in new_rows..self.rows {
                    if self.lines.len() > new_rows + self.ybase {
                        if self.lines.len() > self.ybase + self.y + 1 {
                            // A blank line below the cursor.
                            self.lines.pop();
                        } else {
                            self.ybase += 1;
                            self.ydisp += 1;
                        }
                    }
                }
            }
            if new_max_length < self.lines.max_length {
                let amount = self.lines.len() - to_i64(new_max_length);
                if amount > 0 {
                    self.lines.trim_start(to_usize(amount));
                    self.ybase = (self.ybase - amount).max(0);
                    self.ydisp = (self.ydisp - amount).max(0);
                    self.saved_y = (self.saved_y - amount).max(0);
                }
                self.lines.set_max_length(new_max_length);
            }
            self.x = self.x.min(new_cols - 1);
            self.y = self.y.min(new_rows - 1);
            if add_to_y != 0 {
                self.y += add_to_y;
            }
            self.saved_x = self.saved_x.min(new_cols - 1);
            self.scroll_top = 0;
        }
        self.scroll_bottom = new_rows - 1;

        // Reflow is on for the normal buffer only (the alternate one has no scrollback).
        if self.has_scrollback {
            self.reflow(new_cols, new_rows);
            if self.cols > new_cols {
                for index in 0..self.lines.len() {
                    if let Some(line) = self.line(index) {
                        line.borrow_mut().resize(new_cols, &null_cell);
                    }
                }
            }
        }
        self.cols = new_cols;
        self.rows = new_rows;
    }

    fn reflow(&mut self, new_cols: i64, new_rows: i64) {
        if self.cols == new_cols {
            return;
        }
        if new_cols > self.cols {
            self.reflow_larger(new_cols, new_rows);
        } else {
            self.reflow_smaller(new_cols, new_rows);
        }
    }

    fn reflow_larger(&mut self, new_cols: i64, new_rows: i64) {
        let to_remove = reflow_larger_get_lines_to_remove(&self.lines, self.cols, new_cols, self.ybase + self.y);
        if !to_remove.is_empty() {
            let (layout, count_removed) = reflow_larger_create_new_layout(&self.lines, &to_remove);
            reflow_larger_apply_new_layout(&mut self.lines, &layout);
            self.reflow_larger_adjust_viewport(new_cols, new_rows, count_removed);
        }
    }

    fn reflow_larger_adjust_viewport(&mut self, new_cols: i64, new_rows: i64, count_removed: i64) {
        let null_cell = default_null_cell();
        for _ in 0..count_removed {
            if self.ybase == 0 {
                if self.y > 0 {
                    self.y -= 1;
                }
                if self.lines.len() < new_rows {
                    self.lines.push(line_ref(BufferLine::new(new_cols, &null_cell, false)));
                }
            } else {
                if self.ydisp == self.ybase {
                    self.ydisp -= 1;
                }
                self.ybase -= 1;
            }
        }
        self.saved_y = (self.saved_y - count_removed).max(0);
    }

    fn reflow_smaller(&mut self, new_cols: i64, new_rows: i64) {
        let null_cell = default_null_cell();
        let old_cols = self.cols;
        // (start, new lines) to insert once at the end, in ascending order of `start`.
        let mut to_insert: Vec<(i64, Vec<LineRef>)> = Vec::new();
        let mut count_to_insert: i64 = 0;
        let mut y = self.lines.len() - 1;
        while y >= 0 {
            let Some(mut next_line) = self.line(y) else {
                y -= 1;
                continue;
            };
            let (wrapped, trimmed) = {
                let line = next_line.borrow();
                (line.is_wrapped, line.get_trimmed_length())
            };
            if !wrapped && trimmed <= new_cols {
                y -= 1;
                continue;
            }

            // Gather the wrapped lines; `y` becomes the first of them.
            let mut wrapped_lines: Vec<LineRef> = vec![next_line.clone()];
            let mut broken = false;
            while next_line.borrow().is_wrapped && y > 0 {
                y -= 1;
                match self.line(y) {
                    Some(line) => {
                        next_line = line;
                        wrapped_lines.insert(0, next_line.clone());
                    }
                    None => {
                        broken = true;
                        break;
                    }
                }
            }
            if broken {
                break;
            }

            // Lines holding the cursor are left alone: the program fixes them up itself.
            let absolute_y = self.ybase + self.y;
            let original_count = to_i64(wrapped_lines.len());
            if absolute_y >= y && absolute_y < y + original_count {
                y -= 1;
                continue;
            }

            let last_line_length = wrapped_lines
                .last()
                .map_or(0, |line| line.borrow().get_trimmed_length());
            let dest_line_lengths = reflow_smaller_get_new_line_lengths(&wrapped_lines, old_cols, new_cols);
            let lines_to_add = to_i64(dest_line_lengths.len()) - original_count;
            let trimmed_lines = if self.ybase == 0 && self.y != self.lines.len() - 1 {
                (self.y - to_i64(self.lines.max_length) + lines_to_add).max(0)
            } else {
                (self.lines.len() - to_i64(self.lines.max_length) + lines_to_add).max(0)
            };

            let mut new_lines: Vec<LineRef> = Vec::new();
            for _ in 0..lines_to_add {
                new_lines.push(line_ref(BufferLine::new(new_cols, &null_cell, true)));
            }
            if !new_lines.is_empty() {
                to_insert.push((y + original_count + count_to_insert, new_lines.clone()));
                count_to_insert += to_i64(new_lines.len());
            }
            wrapped_lines.extend(new_lines);

            // Copy cells to their new places, backwards so it can happen in place.
            let lengths = |index: i64| {
                usize::try_from(index)
                    .ok()
                    .and_then(|index| dest_line_lengths.get(index).copied())
            };
            let wrapped_at = |index: i64| {
                usize::try_from(index)
                    .ok()
                    .and_then(|index| wrapped_lines.get(index).cloned())
            };
            let mut dest_line_index = to_i64(dest_line_lengths.len()) - 1;
            let mut dest_col = lengths(dest_line_index);
            if dest_col == Some(0) {
                dest_line_index -= 1;
                dest_col = lengths(dest_line_index);
            }
            let mut src_line_index = to_i64(wrapped_lines.len()) - lines_to_add - 1;
            let mut src_col = last_line_length;
            while src_line_index >= 0 {
                // xterm stops at a missing destination line (its sanity check) and throws on a missing source.
                let (Some(dest), Some(dest_col_value)) = (wrapped_at(dest_line_index), dest_col) else {
                    break;
                };
                let Some(src) = wrapped_at(src_line_index) else { break };
                let cells_to_copy = src_col.min(dest_col_value);
                copy_cells(
                    &dest,
                    &src,
                    src_col - cells_to_copy,
                    dest_col_value - cells_to_copy,
                    cells_to_copy,
                    true,
                );
                dest_col = Some(dest_col_value - cells_to_copy);
                if dest_col == Some(0) {
                    dest_line_index -= 1;
                    dest_col = lengths(dest_line_index);
                }
                src_col -= cells_to_copy;
                if src_col == 0 {
                    src_line_index -= 1;
                    let wrapped_lines_index = src_line_index.max(0);
                    src_col = wrapped_line_trimmed_length(&wrapped_lines, wrapped_lines_index, old_cols);
                }
            }

            // Null out the line ends where a wide character moved to the next line.
            for (index, line) in wrapped_lines.iter().enumerate() {
                if let Some(&length) = dest_line_lengths.get(index)
                    && length < new_cols
                {
                    line.borrow_mut().set_cell(length, &null_cell);
                }
            }

            let mut viewport_adjustments = lines_to_add - trimmed_lines;
            while viewport_adjustments > 0 {
                viewport_adjustments -= 1;
                if self.ybase == 0 {
                    if self.y < new_rows - 1 {
                        self.y += 1;
                        self.lines.pop();
                    } else {
                        self.ybase += 1;
                        self.ydisp += 1;
                    }
                } else if self.ybase < to_i64(self.lines.max_length).min(self.lines.len() + count_to_insert) - new_rows
                {
                    if self.ybase == self.ydisp {
                        self.ydisp += 1;
                    }
                    self.ybase += 1;
                }
            }
            self.saved_y = (self.saved_y + lines_to_add).min(self.ybase + new_rows - 1);
            y -= 1;
        }

        // Insert the new lines in one pass.
        if !to_insert.is_empty() {
            let original_lines: Vec<Option<LineRef>> = (0..self.lines.len()).map(|index| self.line(index)).collect();
            let original_length = self.lines.len();
            let mut original_index = original_length - 1;
            let mut next_index = 0;
            let max_length = to_i64(self.lines.max_length);
            self.lines
                .set_length(to_usize(max_length.min(original_length + count_to_insert)));
            let mut inserted_so_far = 0;
            let mut i = (max_length - 1).min(original_length + count_to_insert - 1);
            while i >= 0 {
                match to_insert.get(next_index) {
                    Some((start, new_lines)) if *start > original_index + inserted_so_far => {
                        for line in new_lines.iter().rev() {
                            self.lines.set(i, Some(line.clone()));
                            i -= 1;
                        }
                        i += 1;
                        inserted_so_far += to_i64(new_lines.len());
                        next_index += 1;
                    }
                    _ => {
                        let line = usize::try_from(original_index)
                            .ok()
                            .and_then(|index| original_lines.get(index).cloned())
                            .flatten();
                        self.lines.set(i, line);
                        original_index -= 1;
                    }
                }
                i -= 1;
            }
        }
    }
}

/// TS: getWrappedLineTrimmedLength
fn wrapped_line_trimmed_length(lines: &[LineRef], index: i64, cols: i64) -> i64 {
    let Some(line) = usize::try_from(index).ok().and_then(|index| lines.get(index)) else {
        return 0;
    };
    let line = line.borrow();
    if index == to_i64(lines.len()) - 1 {
        return line.get_trimmed_length();
    }
    let ends_in_null = line.has_content(cols - 1) == 0 && line.get_width(cols - 1) == 1;
    let following_starts_wide = usize::try_from(index + 1)
        .ok()
        .and_then(|index| lines.get(index))
        .is_some_and(|next| next.borrow().get_width(0) == 2);
    if ends_in_null && following_starts_wide {
        return cols - 1;
    }
    cols
}

/// TS: reflowSmallerGetNewLineLengths
fn reflow_smaller_get_new_line_lengths(wrapped_lines: &[LineRef], old_cols: i64, new_cols: i64) -> Vec<i64> {
    let mut lengths = Vec::new();
    let cells_needed: i64 = (0..wrapped_lines.len())
        .map(|index| wrapped_line_trimmed_length(wrapped_lines, to_i64(index), old_cols))
        .sum();
    let mut src_col = 0;
    let mut src_line = 0;
    let mut cells_available = 0;
    while cells_available < cells_needed {
        if cells_needed - cells_available < new_cols {
            lengths.push(cells_needed - cells_available);
            break;
        }
        src_col += new_cols;
        let old_trimmed_length = wrapped_line_trimmed_length(wrapped_lines, src_line, old_cols);
        if src_col > old_trimmed_length {
            src_col -= old_trimmed_length;
            src_line += 1;
        }
        let Some(line) = usize::try_from(src_line)
            .ok()
            .and_then(|index| wrapped_lines.get(index))
        else {
            break;
        };
        let ends_with_wide = line.borrow().get_width(src_col - 1) == 2;
        if ends_with_wide {
            src_col -= 1;
        }
        let line_length = if ends_with_wide { new_cols - 1 } else { new_cols };
        lengths.push(line_length);
        cells_available += line_length;
    }
    lengths
}

/// TS: reflowLargerGetLinesToRemove — flat `[index, count, ...]` pairs.
fn reflow_larger_get_lines_to_remove(lines: &CircularList, old_cols: i64, new_cols: i64, absolute_y: i64) -> Vec<i64> {
    let null_cell = default_null_cell();
    let mut to_remove = Vec::new();
    let mut y = 0;
    while y < lines.len() - 1 {
        let mut i = y + 1;
        let Some(mut next_line) = lines.get(i) else { break };
        if !next_line.borrow().is_wrapped {
            y += 1;
            continue;
        }
        let Some(first) = lines.get(y) else { break };
        let mut wrapped_lines = vec![first];
        while i < lines.len() && next_line.borrow().is_wrapped {
            wrapped_lines.push(next_line.clone());
            i += 1;
            match lines.get(i) {
                Some(line) => next_line = line,
                None => break,
            }
        }
        let count = to_i64(wrapped_lines.len());
        if absolute_y >= y && absolute_y < i {
            y += count;
            continue;
        }

        let at = |index: i64| {
            usize::try_from(index)
                .ok()
                .and_then(|index| wrapped_lines.get(index).cloned())
        };
        let mut dest_line_index = 0;
        let mut dest_col = wrapped_line_trimmed_length(&wrapped_lines, dest_line_index, old_cols);
        let mut src_line_index = 1;
        let mut src_col = 0;
        while src_line_index < count {
            let src_trimmed_length = wrapped_line_trimmed_length(&wrapped_lines, src_line_index, old_cols);
            let src_remaining = src_trimmed_length - src_col;
            let dest_remaining = new_cols - dest_col;
            let cells_to_copy = src_remaining.min(dest_remaining);
            if let (Some(dest), Some(src)) = (at(dest_line_index), at(src_line_index)) {
                copy_cells(&dest, &src, src_col, dest_col, cells_to_copy, false);
            }
            dest_col += cells_to_copy;
            if dest_col == new_cols {
                dest_line_index += 1;
                dest_col = 0;
            }
            src_col += cells_to_copy;
            if src_col == src_trimmed_length {
                src_line_index += 1;
                src_col = 0;
            }
            // A wide character in the last column moves on to the current line.
            if dest_col == 0
                && dest_line_index != 0
                && let (Some(previous), Some(dest)) = (at(dest_line_index - 1), at(dest_line_index))
                && previous.borrow().get_width(new_cols - 1) == 2
            {
                copy_cells(&dest, &previous, new_cols - 1, dest_col, 1, false);
                dest_col += 1;
                previous.borrow_mut().set_cell(new_cols - 1, &null_cell);
            }
        }
        if let Some(dest) = at(dest_line_index) {
            dest.borrow_mut().replace_cells(dest_col, new_cols, &null_cell, false);
        }

        // Rows at the end that are now empty go.
        let mut count_to_remove = 0;
        let mut index = count - 1;
        while index > 0 {
            let empty = at(index).is_some_and(|line| line.borrow().get_trimmed_length() == 0);
            if index > dest_line_index || empty {
                count_to_remove += 1;
            } else {
                break;
            }
            index -= 1;
        }
        if count_to_remove > 0 {
            to_remove.push(y + count - count_to_remove);
            to_remove.push(count_to_remove);
        }
        y += count;
    }
    to_remove
}

/// TS: reflowLargerCreateNewLayout — the surviving line indexes and how many were removed.
fn reflow_larger_create_new_layout(lines: &CircularList, to_remove: &[i64]) -> (Vec<i64>, i64) {
    let mut layout = Vec::new();
    let mut next_index = 0;
    let mut next_start = to_remove.first().copied();
    let mut count_removed = 0;
    let mut i = 0;
    while i < lines.len() {
        if next_start == Some(i) {
            next_index += 1;
            let count = to_remove.get(next_index).copied().unwrap_or(0);
            i += count - 1;
            count_removed += count;
            next_index += 1;
            next_start = to_remove.get(next_index).copied();
        } else {
            layout.push(i);
        }
        i += 1;
    }
    (layout, count_removed)
}

/// TS: reflowLargerApplyNewLayout
fn reflow_larger_apply_new_layout(lines: &mut CircularList, layout: &[i64]) {
    let new_lines: Vec<Option<LineRef>> = layout.iter().map(|index| lines.get(*index)).collect();
    for (index, line) in (0..).zip(new_lines) {
        lines.set(index, line);
    }
    lines.set_length(layout.len());
}

// ── Core (CoreTerminal + InputHandler + services) ────────────────────────────────────────────────────────────────

/// TS: CoreService.decPrivateModes
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DecModes {
    application_cursor_keys: bool,
    application_keypad: bool,
    bracketed_paste_mode: bool,
    origin: bool,
    reverse_wraparound: bool,
    send_focus: bool,
    wraparound: bool,
    /// DECTCEM reset (`?25l`). xterm.js keeps this in CoreService.isCursorHidden, outside `terminal.modes`.
    cursor_hidden: bool,
}

impl Default for DecModes {
    fn default() -> Self {
        DecModes {
            application_cursor_keys: false,
            application_keypad: false,
            bracketed_paste_mode: false,
            origin: false,
            reverse_wraparound: false,
            send_focus: false,
            wraparound: true,
            cursor_hidden: false,
        }
    }
}

/// The xterm.js core: buffers, modes, charsets and the parser feeding InputHandler.
struct Core {
    cols: i64,
    rows: i64,
    scrollback: usize,
    normal: Buffer,
    alt: Buffer,
    alt_active: bool,
    cur_attr: Attr,
    erase_attr: Attr,
    insert_mode: bool,
    convert_eol: bool,
    dec: DecModes,
    mouse_protocol: MouseTrackingMode,
    charset: Charset,
    glevel: usize,
    charsets: [Charset; 4],
    next_link_id: u32,
    events: EventSink,
    parser: Parser,
}

/// TS: CSI/ESC handler identifier `collect << 8 | final`.
const fn ident(collect: &[u8], final_byte: u8) -> u32 {
    let mut value: u32 = 0;
    let mut index = 0;
    while index < collect.len() {
        value = (value << 8) | collect[index] as u32;
        index += 1;
    }
    (value << 8) | final_byte as u32
}

impl Core {
    fn new(cols: i64, rows: i64, scrollback: usize, events: EventSink) -> Self {
        let cols = cols.max(MINIMUM_COLS);
        let rows = rows.max(MINIMUM_ROWS);
        let mut core = Core {
            cols,
            rows,
            scrollback,
            normal: Buffer::new(true, cols, rows, scrollback),
            alt: Buffer::new(false, cols, rows, scrollback),
            alt_active: false,
            cur_attr: Attr::default(),
            erase_attr: Attr::default(),
            insert_mode: false,
            convert_eol: false,
            dec: DecModes::default(),
            mouse_protocol: MouseTrackingMode::None,
            charset: None,
            glevel: 0,
            charsets: [None; 4],
            next_link_id: 1,
            events,
            parser: Parser::new(),
        };
        core.reset_buffers();
        core
    }

    fn active(&self) -> &Buffer {
        if self.alt_active { &self.alt } else { &self.normal }
    }

    fn buf(&mut self) -> &mut Buffer {
        if self.alt_active {
            &mut self.alt
        } else {
            &mut self.normal
        }
    }

    /// TS: BufferSet.reset
    fn reset_buffers(&mut self) {
        self.normal = Buffer::new(true, self.cols, self.rows, self.scrollback);
        self.normal.fill_viewport_rows(self.cols, Attr::default());
        self.alt = Buffer::new(false, self.cols, self.rows, self.scrollback);
        self.alt_active = false;
        self.normal.setup_tab_stops(None);
        self.alt.setup_tab_stops(None);
    }

    /// TS: Terminal.reset (after RIS)
    fn reset(&mut self) {
        self.cur_attr = Attr::default();
        self.erase_attr = Attr::default();
        self.reset_buffers();
        self.charset = None;
        self.charsets = [None; 4];
        self.glevel = 0;
        self.insert_mode = false;
        self.dec = DecModes::default();
        self.mouse_protocol = MouseTrackingMode::None;
    }

    /// TS: Terminal.resize (headless) → CoreTerminal.resize → BufferService.resize
    fn resize(&mut self, cols: i64, rows: i64) {
        if cols == self.cols && rows == self.rows {
            return;
        }
        let cols = cols.max(MINIMUM_COLS);
        let rows = rows.max(MINIMUM_ROWS);
        self.cols = cols;
        self.rows = rows;
        self.normal.resize(cols, rows);
        self.alt.resize(cols, rows);
        self.normal.setup_tab_stops(Some(cols));
        self.alt.setup_tab_stops(Some(cols));
    }

    /// TS: InputHandler.parse(string) — decoded to code points (U+FEFF dropped), in chunks of MAX_PARSEBUFFER_LENGTH UTF-16 units.
    fn write(&mut self, text: &str) {
        let mut chunk: Vec<u32> = Vec::new();
        let mut units = 0;
        let mut chunk_end = MAX_PARSEBUFFER_LENGTH;
        for ch in text.chars() {
            // A surrogate pair straddling a chunk boundary is completed (and parsed) with the next chunk.
            if units + ch.len_utf16() > chunk_end {
                self.parse(&chunk);
                chunk.clear();
                chunk_end += MAX_PARSEBUFFER_LENGTH;
            }
            // TS: StringToUtf32.decode — drops every U+FEFF (BOM / ZWNBSP); it still counts toward the chunk.
            if ch != '\u{FEFF}' {
                chunk.push(u32::from(ch));
            }
            units += ch.len_utf16();
        }
        if !chunk.is_empty() {
            self.parse(&chunk);
        }
    }

    /// TS: EscapeSequenceParser.parse
    fn parse(&mut self, data: &[u32]) {
        let table = transition_table();
        let length = data.len();
        let mut i = 0;
        while i < length {
            let mut code = data.get(i).copied().unwrap_or(0);
            let index = (usize::from(self.parser.state) << 8) | code.min(NON_ASCII_PRINTABLE) as usize;
            let mut transition = table.get(index).copied().unwrap_or(0);
            match transition >> 4 {
                action::PRINT => {
                    let mut j = i + 1;
                    while j < length {
                        code = data.get(j).copied().unwrap_or(0);
                        if code < 0x20 || (code > 0x7e && code < NON_ASCII_PRINTABLE) {
                            break;
                        }
                        j += 1;
                    }
                    if self.print(data, i, j).is_err() {
                        return;
                    }
                    i = j - 1;
                }
                action::EXECUTE => {
                    if self.execute(code).is_err() {
                        return;
                    }
                    self.parser.preceding_join_state = 0;
                }
                action::CSI_DISPATCH => {
                    let ident = (self.parser.collect << 8) | code;
                    let params = self.parser.params.clone();
                    if self.csi_dispatch(ident, &params).is_err() {
                        return;
                    }
                    self.parser.preceding_join_state = 0;
                }
                action::PARAM => {
                    loop {
                        match code {
                            0x3b => self.parser.params.add_param(0),
                            0x3a => self.parser.params.add_sub_param(-1),
                            _ => self.parser.params.add_digit(code as i32 - 48),
                        }
                        i += 1;
                        if i < length {
                            code = data.get(i).copied().unwrap_or(0);
                            if code > 0x2f && code < 0x3c {
                                continue;
                            }
                        }
                        break;
                    }
                    i -= 1;
                }
                action::COLLECT => {
                    self.parser.collect = (self.parser.collect << 8) | code;
                }
                action::ESC_DISPATCH => {
                    let ident = (self.parser.collect << 8) | code;
                    if self.esc_dispatch(ident).is_err() {
                        return;
                    }
                    self.parser.preceding_join_state = 0;
                }
                action::CLEAR => self.parser.clear(),
                action::DCS_PUT => {
                    // DCS payloads only feed DECRQSS, whose reply is swallowed.
                    let mut j = i + 1;
                    while j < length {
                        code = data.get(j).copied().unwrap_or(0);
                        if code == 0x18 || code == 0x1a || code == 0x1b || (code > 0x7f && code < NON_ASCII_PRINTABLE) {
                            break;
                        }
                        j += 1;
                    }
                    i = j - 1;
                }
                action::DCS_UNHOOK => {
                    if code == 0x1b {
                        transition |= state::ESCAPE;
                    }
                    self.parser.clear();
                    self.parser.preceding_join_state = 0;
                }
                action::OSC_START => self.parser.osc.start(),
                action::OSC_PUT => {
                    let mut j = i + 1;
                    while j < length {
                        code = data.get(j).copied().unwrap_or(0);
                        if code < 0x20 || (code > 0x7f && code < NON_ASCII_PRINTABLE) {
                            break;
                        }
                        j += 1;
                    }
                    self.parser.osc.put(data, i, j);
                    i = j - 1;
                }
                action::OSC_END => {
                    let success = code != 0x18 && code != 0x1a;
                    if let Some((id, payload)) = self.parser.osc.end(success) {
                        self.osc_dispatch(id, &payload);
                    }
                    if code == 0x1b {
                        transition |= state::ESCAPE;
                    }
                    self.parser.clear();
                    self.parser.preceding_join_state = 0;
                }
                // IGNORE, ERROR (the error handler only logs) and DCS_HOOK (no visible DCS handler).
                _ => {}
            }
            self.parser.state = transition & 0x0F;
            i += 1;
        }
    }

    fn erase_attr_data(&mut self) -> Attr {
        self.erase_attr.bg &= !(CM_MASK | 0xFF_FFFF);
        self.erase_attr.bg |= self.cur_attr.bg & !0xFC00_0000;
        self.erase_attr
    }

    fn erase_cell(&mut self) -> CellData {
        CellData::null(self.erase_attr_data())
    }

    fn current_line(&self) -> Option<LineRef> {
        let buffer = self.active();
        buffer.line(buffer.ybase + buffer.y)
    }

    // ── print ────────────────────────────────────────────────────────────────────────────────────────────────────

    /// TS: InputHandler.print
    fn print(&mut self, data: &[u32], start: usize, end: usize) -> Step {
        let charset = self.charset;
        let cols = self.cols;
        let rows = self.rows;
        let wraparound = self.dec.wraparound;
        let insert_mode = self.insert_mode;
        let cur = self.cur_attr;
        let mut row = self.current_line().ok_or(Abort)?;

        // A print over the second half of a wide character clears its first half.
        let x = self.active().x;
        if x != 0 && end > start && row.borrow().get_width(x - 1) == 2 {
            row.borrow_mut().set_cell_from_codepoint(x - 1, 0, 1, &cur);
        }

        let mut preceding = self.parser.preceding_join_state;
        for pos in start..end {
            let mut code = data.get(pos).copied().unwrap_or(0);
            if code < 127
                && let Some(flag) = charset
                && let Some(mapped) = charset_map(flag, code)
            {
                code = u32::from(mapped);
            }
            let info = char_properties(code, preceding);
            let mut ch_width = i64::from(extract_width(info));
            let should_join = extract_should_join(info);
            let old_width = if should_join {
                i64::from(extract_width(preceding))
            } else {
                0
            };
            preceding = info;

            if self.active().x + ch_width - old_width > cols {
                if wraparound {
                    let old_row = row.clone();
                    let mut old_col = self.active().x - old_width;
                    if self.advance_line(rows) == LineMove::Scroll {
                        self.scroll(true)?;
                    } else {
                        // The line already exists (e.g. the initial viewport): mark it wrapped.
                        let line = self.current_line().ok_or(Abort)?;
                        line.borrow_mut().is_wrapped = true;
                    }
                    self.buf().x = old_width;
                    row = self.current_line().ok_or(Abort)?;
                    if old_width > 0 {
                        // A combining character widened the previous one: move it to the next line.
                        copy_cells(&row, &old_row, old_col, 0, old_width, false);
                    }
                    while old_col < cols {
                        old_row.borrow_mut().set_cell_from_codepoint(old_col, 0, 1, &cur);
                        old_col += 1;
                    }
                } else {
                    self.buf().x = cols - 1;
                    if ch_width == 2 {
                        continue;
                    }
                }
            }

            let x = self.active().x;
            if should_join && x != 0 {
                let offset = if row.borrow().get_width(x - 1) != 0 { 1 } else { 2 };
                let mut line = row.borrow_mut();
                line.add_codepoint_to_cell(x - offset, code, ch_width as u32);
                let mut x = x;
                for _ in 0..(ch_width - old_width).max(0) {
                    line.set_cell_from_codepoint(x, 0, 0, &cur);
                    x += 1;
                }
                drop(line);
                self.buf().x = x;
                continue;
            }

            let mut line = row.borrow_mut();
            if insert_mode {
                line.insert_cells(x, ch_width - old_width, &CellData::null(cur));
                if line.get_width(cols - 1) == 2 {
                    line.set_cell_from_codepoint(cols - 1, 0, 1, &cur);
                }
            }
            let mut x = x;
            line.set_cell_from_codepoint(x, code, ch_width as u32, &cur);
            x += 1;
            if ch_width > 0 {
                while {
                    ch_width -= 1;
                    ch_width != 0
                } {
                    line.set_cell_from_codepoint(x, 0, 0, &cur);
                    x += 1;
                }
            }
            drop(line);
            self.buf().x = x;
        }
        self.parser.preceding_join_state = preceding;

        // A print ending on the first half of a wide character leaves its second half empty.
        let x = self.active().x;
        let mut line = row.borrow_mut();
        if x < cols && end > start && line.get_width(x) == 0 && line.has_content(x) == 0 {
            line.set_cell_from_codepoint(x, 0, 1, &cur);
        }
        Ok(())
    }

    /// TS: BufferService.scroll
    fn scroll(&mut self, is_wrapped: bool) -> Step {
        let erase = self.erase_attr_data();
        let cols = self.cols;
        let buffer = self.buf();
        let new_line = BufferLine::new(cols, &CellData::null(erase), is_wrapped);
        let top_row = buffer.ybase + buffer.scroll_top;
        let bottom_row = buffer.ybase + buffer.scroll_bottom;
        if buffer.scroll_top == 0 {
            let will_be_trimmed = buffer.lines.is_full();
            if bottom_row == buffer.lines.len() - 1 {
                if will_be_trimmed {
                    let line = buffer.lines.recycle().ok_or(Abort)?;
                    line.borrow_mut().copy_from(&new_line);
                } else {
                    buffer.lines.push(line_ref(new_line));
                }
            } else {
                buffer.lines.splice(bottom_row + 1, 0, vec![line_ref(new_line)]);
            }
            if !will_be_trimmed {
                buffer.ybase += 1;
                buffer.ydisp += 1;
            }
        } else {
            let region_height = bottom_row - top_row + 1;
            buffer.lines.shift_elements(top_row + 1, region_height - 1, -1)?;
            buffer.lines.set(bottom_row, Some(line_ref(new_line)));
        }
        buffer.ydisp = buffer.ybase;
        Ok(())
    }

    // ── execute ──────────────────────────────────────────────────────────────────────────────────────────────────

    fn execute(&mut self, code: u32) -> Step {
        match code {
            0x0a..=0x0c => self.line_feed(),
            0x0d => {
                self.buf().x = 0;
                Ok(())
            }
            0x08 => {
                self.backspace();
                Ok(())
            }
            0x09 => {
                self.tab();
                Ok(())
            }
            0x0e => {
                self.set_glevel(1);
                Ok(())
            }
            0x0f => {
                self.set_glevel(0);
                Ok(())
            }
            0x84 => self.index(),
            0x85 => self.next_line(),
            0x88 => {
                self.tab_set();
                Ok(())
            }
            // BEL and the rest have no visible effect.
            _ => Ok(()),
        }
    }

    /// `y += 1`, then the scroll-region bottom scrolls and the screen bottom clamps (the shared head of lineFeed,
    /// index and print's auto-wrap).
    fn advance_line(&mut self, rows: i64) -> LineMove {
        let buffer = self.buf();
        buffer.y += 1;
        if buffer.y == buffer.scroll_bottom + 1 {
            buffer.y -= 1;
            LineMove::Scroll
        } else if buffer.y >= rows {
            buffer.y = rows - 1;
            LineMove::Clamped
        } else {
            LineMove::Moved
        }
    }

    /// TS: InputHandler.lineFeed
    fn line_feed(&mut self) -> Step {
        let rows = self.rows;
        let cols = self.cols;
        if self.convert_eol {
            self.buf().x = 0;
        }
        match self.advance_line(rows) {
            LineMove::Scroll => self.scroll(false)?,
            LineMove::Clamped => {}
            LineMove::Moved => {
                // An explicit line feed ends the wrapped state of the line it moves to.
                let line = self.current_line().ok_or(Abort)?;
                line.borrow_mut().is_wrapped = false;
            }
        }
        let buffer = self.buf();
        if buffer.x >= cols {
            buffer.x -= 1;
        }
        Ok(())
    }

    /// TS: InputHandler.backspace
    fn backspace(&mut self) {
        if !self.dec.reverse_wraparound {
            self.restrict_cursor(None);
            let buffer = self.buf();
            if buffer.x > 0 {
                buffer.x -= 1;
            }
            return;
        }
        let cols = self.cols;
        self.restrict_cursor(Some(cols));
        let buffer = self.buf();
        if buffer.x > 0 {
            buffer.x -= 1;
        } else if buffer.x == 0 && buffer.y > buffer.scroll_top && buffer.y <= buffer.scroll_bottom {
            let line = buffer.line(buffer.ybase + buffer.y);
            if let Some(line) = line
                && line.borrow().is_wrapped
            {
                line.borrow_mut().is_wrapped = false;
                buffer.y -= 1;
                buffer.x = cols - 1;
                if let Some(previous) = buffer.line(buffer.ybase + buffer.y) {
                    let previous = previous.borrow();
                    if previous.has_width(buffer.x) != 0 && previous.has_content(buffer.x) == 0 {
                        buffer.x -= 1;
                    }
                }
            }
        }
        self.restrict_cursor(None);
    }

    /// TS: InputHandler.tab
    fn tab(&mut self) {
        let cols = self.cols;
        let buffer = self.buf();
        if buffer.x >= cols {
            return;
        }
        buffer.x = buffer.next_stop(buffer.x);
    }

    fn set_glevel(&mut self, level: usize) {
        self.glevel = level;
        self.charset = self.charsets.get(level).copied().flatten();
    }

    fn set_gcharset(&mut self, level: usize, charset: Charset) {
        if let Some(slot) = self.charsets.get_mut(level) {
            *slot = charset;
        }
        if self.glevel == level {
            self.charset = charset;
        }
    }

    /// TS: InputHandler._restrictCursor(maxCol = cols - 1)
    fn restrict_cursor(&mut self, max_col: Option<i64>) {
        let max_col = max_col.unwrap_or(self.cols - 1);
        let rows = self.rows;
        let origin = self.dec.origin;
        let buffer = self.buf();
        buffer.x = max_col.min(buffer.x.max(0));
        buffer.y = if origin {
            buffer.scroll_bottom.min(buffer.y.max(buffer.scroll_top))
        } else {
            (rows - 1).min(buffer.y.max(0))
        };
    }

    /// TS: InputHandler._setCursor
    fn set_cursor(&mut self, x: i64, y: i64) {
        let origin = self.dec.origin;
        let buffer = self.buf();
        buffer.x = x;
        buffer.y = if origin { buffer.scroll_top + y } else { y };
        self.restrict_cursor(None);
    }

    /// TS: InputHandler._moveCursor
    fn move_cursor(&mut self, x: i64, y: i64) {
        self.restrict_cursor(None);
        let buffer = self.active();
        let (next_x, next_y) = (buffer.x + x, buffer.y + y);
        self.set_cursor(next_x, next_y);
    }

    /// TS: InputHandler.index
    fn index(&mut self) -> Step {
        self.restrict_cursor(None);
        let rows = self.rows;
        if self.advance_line(rows) == LineMove::Scroll {
            self.scroll(false)?;
        }
        self.restrict_cursor(None);
        Ok(())
    }

    fn next_line(&mut self) -> Step {
        self.buf().x = 0;
        self.index()
    }

    fn tab_set(&mut self) {
        let buffer = self.buf();
        buffer.tabs.insert(buffer.x);
    }

    /// TS: InputHandler.reverseIndex
    fn reverse_index(&mut self) -> Step {
        self.restrict_cursor(None);
        let erase = self.erase_attr_data();
        let cols = self.cols;
        let buffer = self.buf();
        if buffer.y == buffer.scroll_top {
            let region_height = buffer.scroll_bottom - buffer.scroll_top;
            let row = buffer.ybase + buffer.y;
            buffer.lines.shift_elements(row, region_height, 1)?;
            buffer.lines.set(
                row,
                Some(line_ref(BufferLine::new(cols, &CellData::null(erase), false))),
            );
        } else {
            buffer.y -= 1;
            self.restrict_cursor(None);
        }
        Ok(())
    }

    // ── OSC ──────────────────────────────────────────────────────────────────────────────────────────────────────

    fn osc_dispatch(&mut self, id: i64, data: &str) {
        match id {
            0 | 2 => self.events.send_event(Event::Title(data.to_string())),
            8 => self.set_hyperlink(data),
            // Icon names and colour requests have no visible effect.
            _ => {}
        }
    }

    /// TS: InputHandler.setHyperlink — only whether a link is open is visible (its cells report an underline).
    fn set_hyperlink(&mut self, data: &str) {
        let args: Vec<&str> = data.split(';').collect();
        let (Some(params), Some(uri)) = (args.first(), args.get(1)) else {
            return;
        };
        if !uri.is_empty() {
            if self.cur_attr.extended.url_id != 0 {
                self.finish_hyperlink();
            }
            self.cur_attr.extended.url_id = self.next_link_id;
            self.next_link_id = self.next_link_id.wrapping_add(1).max(1);
            self.cur_attr.update_extended();
            return;
        }
        if !params.is_empty() {
            return;
        }
        self.finish_hyperlink();
    }

    fn finish_hyperlink(&mut self) {
        self.cur_attr.extended.url_id = 0;
        self.cur_attr.update_extended();
    }

    // ── ESC ──────────────────────────────────────────────────────────────────────────────────────────────────────

    fn esc_dispatch(&mut self, id: u32) -> Step {
        const SAVE: u32 = ident(b"", b'7');
        const RESTORE: u32 = ident(b"", b'8');
        const IND: u32 = ident(b"", b'D');
        const NEL: u32 = ident(b"", b'E');
        const HTS: u32 = ident(b"", b'H');
        const RI: u32 = ident(b"", b'M');
        const DECKPAM: u32 = ident(b"", b'=');
        const DECKPNM: u32 = ident(b"", b'>');
        const RIS: u32 = ident(b"", b'c');
        const LS2: u32 = ident(b"", b'n');
        const LS3: u32 = ident(b"", b'o');
        const LS3R: u32 = ident(b"", b'|');
        const LS2R: u32 = ident(b"", b'}');
        const LS1R: u32 = ident(b"", b'~');
        const CS_DEFAULT_AT: u32 = ident(b"%", b'@');
        const CS_DEFAULT_G: u32 = ident(b"%", b'G');
        const DECALN: u32 = ident(b"#", b'8');
        match id {
            SAVE => self.save_cursor(),
            RESTORE => self.restore_cursor(),
            IND => self.index()?,
            NEL => self.next_line()?,
            HTS => self.tab_set(),
            RI => self.reverse_index()?,
            DECKPAM => self.dec.application_keypad = true,
            DECKPNM => self.dec.application_keypad = false,
            RIS => {
                self.parser.reset();
                self.reset();
            }
            LS2 | LS2R => self.set_glevel(2),
            LS3 | LS3R => self.set_glevel(3),
            LS1R => self.set_glevel(1),
            CS_DEFAULT_AT | CS_DEFAULT_G => self.select_default_charset(),
            DECALN => self.screen_alignment_pattern(),
            _ => {
                let intermediate = (id >> 8) as u8;
                let flag = (id & 0xFF) as u8;
                if id >> 16 == 0 && CHARSET_FLAGS.contains(&flag) {
                    let level = match intermediate {
                        b'(' => Some(0),
                        b')' | b'-' => Some(1),
                        b'*' | b'.' => Some(2),
                        b'+' => Some(3),
                        // `/` (96-character G1 designation) is accepted and ignored.
                        _ => None,
                    };
                    if let Some(level) = level {
                        self.set_gcharset(level, charset_for(flag));
                    }
                }
                // Everything else, including the swallowed 7-bit ST (ESC \), is a no-op.
            }
        }
        Ok(())
    }

    fn select_default_charset(&mut self) {
        self.set_glevel(0);
        self.set_gcharset(0, None);
    }

    /// TS: InputHandler.saveCursor (DECSC / SCOSC)
    fn save_cursor(&mut self) {
        let cur = self.cur_attr;
        let charset = self.charset;
        let buffer = self.buf();
        buffer.saved_x = buffer.x;
        buffer.saved_y = buffer.ybase + buffer.y;
        buffer.saved_attr.fg = cur.fg;
        buffer.saved_attr.bg = cur.bg;
        buffer.saved_charset = charset;
    }

    /// TS: InputHandler.restoreCursor (DECRC / SCORC)
    fn restore_cursor(&mut self) {
        let buffer = self.buf();
        buffer.x = buffer.saved_x;
        buffer.y = (buffer.saved_y - buffer.ybase).max(0);
        let saved = buffer.saved_attr;
        let charset = buffer.saved_charset;
        self.cur_attr.fg = saved.fg;
        self.cur_attr.bg = saved.bg;
        self.charset = charset;
        self.restrict_cursor(None);
    }

    /// TS: InputHandler.screenAlignmentPattern (DECALN)
    fn screen_alignment_pattern(&mut self) {
        let cell = CellData {
            content: (1 << WIDTH_SHIFT) | u32::from(b'E'),
            attr: Attr {
                fg: self.cur_attr.fg,
                bg: self.cur_attr.bg,
                extended: ExtendedAttrs::default(),
            },
            combined: String::new(),
        };
        self.set_cursor(0, 0);
        let rows = self.rows;
        let buffer = self.active();
        for offset in 0..rows {
            if let Some(line) = buffer.line(buffer.ybase + buffer.y + offset) {
                let mut line = line.borrow_mut();
                line.fill(&cell, false);
                line.is_wrapped = false;
            }
        }
        self.set_cursor(0, 0);
    }

    // ── CSI ──────────────────────────────────────────────────────────────────────────────────────────────────────

    fn csi_dispatch(&mut self, id: u32, params: &Params) -> Step {
        const ICH: u32 = ident(b"", b'@');
        const SL: u32 = ident(b" ", b'@');
        const CUU: u32 = ident(b"", b'A');
        const SR: u32 = ident(b" ", b'A');
        const CUD: u32 = ident(b"", b'B');
        const CUF: u32 = ident(b"", b'C');
        const CUB: u32 = ident(b"", b'D');
        const CNL: u32 = ident(b"", b'E');
        const CPL: u32 = ident(b"", b'F');
        const CHA: u32 = ident(b"", b'G');
        const CUP: u32 = ident(b"", b'H');
        const CHT: u32 = ident(b"", b'I');
        const ED: u32 = ident(b"", b'J');
        const DECSED: u32 = ident(b"?", b'J');
        const EL: u32 = ident(b"", b'K');
        const DECSEL: u32 = ident(b"?", b'K');
        const IL: u32 = ident(b"", b'L');
        const DL: u32 = ident(b"", b'M');
        const DCH: u32 = ident(b"", b'P');
        const SU: u32 = ident(b"", b'S');
        const SD: u32 = ident(b"", b'T');
        const ECH: u32 = ident(b"", b'X');
        const CBT: u32 = ident(b"", b'Z');
        const HPA: u32 = ident(b"", b'`');
        const HPR: u32 = ident(b"", b'a');
        const REP: u32 = ident(b"", b'b');
        const VPA: u32 = ident(b"", b'd');
        const VPR: u32 = ident(b"", b'e');
        const HVP: u32 = ident(b"", b'f');
        const TBC: u32 = ident(b"", b'g');
        const SM: u32 = ident(b"", b'h');
        const DECSET: u32 = ident(b"?", b'h');
        const RM: u32 = ident(b"", b'l');
        const DECRST: u32 = ident(b"?", b'l');
        const SGR: u32 = ident(b"", b'm');
        const DECSTR: u32 = ident(b"!", b'p');
        const DECSTBM: u32 = ident(b"", b'r');
        const SCOSC: u32 = ident(b"", b's');
        const SCORC: u32 = ident(b"", b'u');
        const DECIC: u32 = ident(b"'", b'}');
        const DECDC: u32 = ident(b"'", b'~');
        const DECSCA: u32 = ident(b"\"", b'q');
        match id {
            ICH => self.insert_chars(params),
            SL => self.scroll_columns(params, ColumnOp::DeleteAtStart)?,
            CUU => self.cursor_up(params),
            SR => self.scroll_columns(params, ColumnOp::InsertAtStart)?,
            CUD => self.cursor_down(params),
            CUF | HPR => self.move_cursor(params.param_or(0, 1), 0),
            CUB => self.move_cursor(-params.param_or(0, 1), 0),
            CNL => {
                self.cursor_down(params);
                self.buf().x = 0;
            }
            CPL => {
                self.cursor_up(params);
                self.buf().x = 0;
            }
            CHA | HPA => {
                let y = self.active().y;
                self.set_cursor(params.param_or(0, 1) - 1, y);
            }
            CUP | HVP => self.cursor_position(params),
            CHT => self.cursor_forward_tab(params),
            ED => self.erase_in_display(params, false)?,
            DECSED => self.erase_in_display(params, true)?,
            EL => self.erase_in_line(params, false)?,
            DECSEL => self.erase_in_line(params, true)?,
            IL => self.insert_lines(params),
            DL => self.delete_lines(params),
            DCH => self.delete_chars(params),
            SU => self.scroll_up(params),
            SD => self.scroll_down(params),
            ECH => self.erase_chars(params),
            CBT => self.cursor_backward_tab(params),
            REP => self.repeat_preceding_character(params)?,
            VPA => {
                let x = self.active().x;
                self.set_cursor(x, params.param_or(0, 1) - 1);
            }
            VPR => self.move_cursor(0, params.param_or(0, 1)),
            TBC => self.tab_clear(params),
            SM => self.set_mode(params, true),
            RM => self.set_mode(params, false),
            DECSET => self.set_mode_private(params),
            DECRST => self.reset_mode_private(params),
            SGR => self.char_attributes(params),
            DECSTR => self.soft_reset(),
            DECSTBM => self.set_scroll_region(params),
            SCOSC => self.save_cursor(),
            SCORC => self.restore_cursor(),
            DECIC => self.scroll_columns(params, ColumnOp::InsertAtCursor)?,
            DECDC => self.scroll_columns(params, ColumnOp::DeleteAtCursor)?,
            DECSCA => self.select_protected(params),
            // DA, DSR, DECRQM, DECSCUSR and window operations only reply or change options nothing reads.
            _ => {}
        }
        Ok(())
    }

    fn cursor_up(&mut self, params: &Params) {
        let buffer = self.active();
        let diff_to_top = buffer.y - buffer.scroll_top;
        if diff_to_top >= 0 {
            self.move_cursor(0, -diff_to_top.min(params.param_or(0, 1)));
        } else {
            self.move_cursor(0, -params.param_or(0, 1));
        }
    }

    fn cursor_down(&mut self, params: &Params) {
        let buffer = self.active();
        let diff_to_bottom = buffer.scroll_bottom - buffer.y;
        if diff_to_bottom >= 0 {
            self.move_cursor(0, diff_to_bottom.min(params.param_or(0, 1)));
        } else {
            self.move_cursor(0, params.param_or(0, 1));
        }
    }

    fn cursor_position(&mut self, params: &Params) {
        let x = if params.length >= 2 {
            params.param_or(1, 1) - 1
        } else {
            0
        };
        self.set_cursor(x, params.param_or(0, 1) - 1);
    }

    fn cursor_forward_tab(&mut self, params: &Params) {
        let cols = self.cols;
        let buffer = self.buf();
        if buffer.x >= cols {
            return;
        }
        // Once at the last stop more iterations change nothing; bound the loop by the width.
        for _ in 0..params.param_or(0, 1).min(cols + 1) {
            buffer.x = buffer.next_stop(buffer.x);
        }
    }

    fn cursor_backward_tab(&mut self, params: &Params) {
        let cols = self.cols;
        let buffer = self.buf();
        if buffer.x >= cols {
            return;
        }
        for _ in 0..params.param_or(0, 1).min(cols + 1) {
            buffer.x = buffer.prev_stop(buffer.x);
        }
    }

    fn tab_clear(&mut self, params: &Params) {
        let buffer = self.buf();
        match params.param(0) {
            0 => {
                buffer.tabs.remove(&buffer.x);
            }
            3 => buffer.tabs.clear(),
            _ => {}
        }
    }

    fn select_protected(&mut self, params: &Params) {
        match params.param(0) {
            1 => self.cur_attr.bg |= BG_PROTECTED,
            0 | 2 => self.cur_attr.bg &= !BG_PROTECTED,
            _ => {}
        }
    }

    /// TS: InputHandler._eraseInBufferLine
    fn erase_in_buffer_line(&mut self, y: i64, start: i64, end: i64, clear_wrap: bool, respect_protect: bool) -> Step {
        let fill = self.erase_cell();
        let buffer = self.active();
        let line = buffer.line(buffer.ybase + y).ok_or(Abort)?;
        let mut line = line.borrow_mut();
        line.replace_cells(start, end, &fill, respect_protect);
        if clear_wrap {
            line.is_wrapped = false;
        }
        Ok(())
    }

    /// TS: InputHandler._resetBufferLine
    fn reset_buffer_line(&mut self, y: i64, respect_protect: bool) {
        let fill = self.erase_cell();
        let buffer = self.active();
        if let Some(line) = buffer.line(buffer.ybase + y) {
            let mut line = line.borrow_mut();
            line.fill(&fill, respect_protect);
            line.is_wrapped = false;
        }
    }

    /// TS: InputHandler.eraseInDisplay
    fn erase_in_display(&mut self, params: &Params, respect_protect: bool) -> Step {
        let cols = self.cols;
        let rows = self.rows;
        self.restrict_cursor(Some(cols));
        let (x, y) = (self.active().x, self.active().y);
        match params.param(0) {
            0 => {
                self.erase_in_buffer_line(y, x, cols, x == 0, respect_protect)?;
                for j in y + 1..rows {
                    self.reset_buffer_line(j, respect_protect);
                }
            }
            1 => {
                self.erase_in_buffer_line(y, 0, x + 1, true, respect_protect)?;
                if x + 1 >= cols {
                    // xterm indexes the buffer with the viewport row here (no ybase).
                    let line = self.active().line(y + 1).ok_or(Abort)?;
                    line.borrow_mut().is_wrapped = false;
                }
                for j in (0..y).rev() {
                    self.reset_buffer_line(j, respect_protect);
                }
            }
            2 => {
                for j in (0..rows).rev() {
                    self.reset_buffer_line(j, respect_protect);
                }
            }
            3 => {
                let buffer = self.buf();
                let scrollback_size = buffer.lines.len() - rows;
                if scrollback_size > 0 {
                    buffer.lines.trim_start(to_usize(scrollback_size));
                    buffer.ybase = (buffer.ybase - scrollback_size).max(0);
                    buffer.ydisp = (buffer.ydisp - scrollback_size).max(0);
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// TS: InputHandler.eraseInLine
    fn erase_in_line(&mut self, params: &Params, respect_protect: bool) -> Step {
        let cols = self.cols;
        self.restrict_cursor(Some(cols));
        let (x, y) = (self.active().x, self.active().y);
        match params.param(0) {
            0 => self.erase_in_buffer_line(y, x, cols, x == 0, respect_protect),
            1 => self.erase_in_buffer_line(y, 0, x + 1, false, respect_protect),
            2 => self.erase_in_buffer_line(y, 0, cols, true, respect_protect),
            _ => Ok(()),
        }
    }

    /// TS: InputHandler.insertLines
    fn insert_lines(&mut self, params: &Params) {
        self.restrict_cursor(None);
        let erase = self.erase_attr_data();
        let (cols, rows) = (self.cols, self.rows);
        let buffer = self.buf();
        if buffer.y > buffer.scroll_bottom || buffer.y < buffer.scroll_top {
            return;
        }
        let row = buffer.ybase + buffer.y;
        let scroll_bottom_rows_offset = rows - 1 - buffer.scroll_bottom;
        let scroll_bottom_absolute = rows - 1 + buffer.ybase - scroll_bottom_rows_offset + 1;
        // After a region's height of iterations every further one swaps a blank line for a blank line.
        let count = params.param_or(0, 1).min(buffer.scroll_bottom - buffer.y + 1);
        for _ in 0..count {
            buffer.lines.splice(scroll_bottom_absolute - 1, 1, Vec::new());
            buffer.lines.splice(
                row,
                0,
                vec![line_ref(BufferLine::new(cols, &CellData::null(erase), false))],
            );
        }
        buffer.x = 0;
    }

    /// TS: InputHandler.deleteLines
    fn delete_lines(&mut self, params: &Params) {
        self.restrict_cursor(None);
        let erase = self.erase_attr_data();
        let (cols, rows) = (self.cols, self.rows);
        let buffer = self.buf();
        if buffer.y > buffer.scroll_bottom || buffer.y < buffer.scroll_top {
            return;
        }
        let row = buffer.ybase + buffer.y;
        let j = rows - 1 + buffer.ybase - (rows - 1 - buffer.scroll_bottom);
        let count = params.param_or(0, 1).min(buffer.scroll_bottom - buffer.y + 1);
        for _ in 0..count {
            buffer.lines.splice(row, 1, Vec::new());
            buffer.lines.splice(
                j,
                0,
                vec![line_ref(BufferLine::new(cols, &CellData::null(erase), false))],
            );
        }
        buffer.x = 0;
    }

    fn insert_chars(&mut self, params: &Params) {
        self.restrict_cursor(None);
        let fill = self.erase_cell();
        let buffer = self.active();
        if let Some(line) = self.current_line() {
            line.borrow_mut().insert_cells(buffer.x, params.param_or(0, 1), &fill);
        }
    }

    fn delete_chars(&mut self, params: &Params) {
        self.restrict_cursor(None);
        let fill = self.erase_cell();
        let buffer = self.active();
        if let Some(line) = self.current_line() {
            line.borrow_mut().delete_cells(buffer.x, params.param_or(0, 1), &fill);
        }
    }

    fn erase_chars(&mut self, params: &Params) {
        self.restrict_cursor(None);
        let fill = self.erase_cell();
        let x = self.active().x;
        if let Some(line) = self.current_line() {
            line.borrow_mut()
                .replace_cells(x, x + params.param_or(0, 1), &fill, false);
        }
    }

    /// TS: InputHandler.scrollUp (SU)
    fn scroll_up(&mut self, params: &Params) {
        let erase = self.erase_attr_data();
        let cols = self.cols;
        let buffer = self.buf();
        let count = params
            .param_or(0, 1)
            .min((buffer.scroll_bottom - buffer.scroll_top + 1).max(1));
        for _ in 0..count {
            buffer.lines.splice(buffer.ybase + buffer.scroll_top, 1, Vec::new());
            buffer.lines.splice(
                buffer.ybase + buffer.scroll_bottom,
                0,
                vec![line_ref(BufferLine::new(cols, &CellData::null(erase), false))],
            );
        }
    }

    /// TS: InputHandler.scrollDown (SD) — new lines take the default attributes, not the erase colour.
    fn scroll_down(&mut self, params: &Params) {
        let cols = self.cols;
        let buffer = self.buf();
        let count = params
            .param_or(0, 1)
            .min((buffer.scroll_bottom - buffer.scroll_top + 1).max(1));
        for _ in 0..count {
            buffer.lines.splice(buffer.ybase + buffer.scroll_bottom, 1, Vec::new());
            buffer.lines.splice(
                buffer.ybase + buffer.scroll_top,
                0,
                vec![line_ref(BufferLine::new(cols, &default_null_cell(), false))],
            );
        }
    }

    /// TS: scrollLeft / scrollRight / insertColumns / deleteColumns
    fn scroll_columns(&mut self, params: &Params, op: ColumnOp) -> Step {
        let fill = self.erase_cell();
        let buffer = self.active();
        if buffer.y > buffer.scroll_bottom || buffer.y < buffer.scroll_top {
            return Ok(());
        }
        let count = params.param_or(0, 1);
        for y in buffer.scroll_top..=buffer.scroll_bottom {
            let line = buffer.line(buffer.ybase + y).ok_or(Abort)?;
            let mut line = line.borrow_mut();
            match op {
                ColumnOp::DeleteAtStart => line.delete_cells(0, count, &fill),
                ColumnOp::InsertAtStart => line.insert_cells(0, count, &fill),
                ColumnOp::InsertAtCursor => line.insert_cells(buffer.x, count, &fill),
                ColumnOp::DeleteAtCursor => line.delete_cells(buffer.x, count, &fill),
            }
            line.is_wrapped = false;
        }
        Ok(())
    }

    /// TS: InputHandler.repeatPrecedingCharacter (REP)
    fn repeat_preceding_character(&mut self, params: &Params) -> Step {
        let join_state = self.parser.preceding_join_state;
        if join_state == 0 {
            return Ok(());
        }
        let length = params.param_or(0, 1);
        let ch_width = i64::from(extract_width(join_state));
        let x = self.active().x - ch_width;
        let line = self.current_line().ok_or(Abort)?;
        let text = line.borrow().get_string(x);
        let unit: Vec<u32> = text.chars().map(u32::from).collect();
        if unit.is_empty() {
            // xterm prints an empty array (after `length` no-op copies); untrusted output must not make us spin.
            return Ok(());
        }
        if to_i64(unit.len()).saturating_mul(length) > MAX_REPEAT_CODEPOINTS {
            return Err(Abort);
        }
        let length = self.equivalent_repeat_count(&unit, length);
        let mut data = Vec::with_capacity(unit.len() * to_usize(length));
        for _ in 0..length {
            data.extend_from_slice(&unit);
        }
        self.print(&data, 0, data.len())
    }

    /// A repeat count no larger than `count` that leaves exactly the state `count` repeats of `unit` would, so REP's
    /// work is bounded by the buffer size rather than by the (untrusted) count.
    ///
    /// Only a single code point of width 1 or 2 is shortened (multi-code-point cells can grow their combined string
    /// without bound when no cell takes them). Such a print never joins the previous cell, so the screen settles into a
    /// cycle: without wraparound the cursor sticks at the right margin after `cols` repeats and each further one
    /// rewrites the same cell; with it, every line takes `cols / width` repeats and after the cursor has reached the
    /// bottom and every line the list can hold has been rewritten (or recycled) the state repeats once per line.
    fn equivalent_repeat_count(&self, unit: &[u32], count: i64) -> i64 {
        let [code] = unit else {
            return count;
        };
        let mut code = *code;
        if code < 127
            && let Some(flag) = self.charset
            && let Some(mapped) = charset_map(flag, code)
        {
            code = u32::from(mapped);
        }
        let width = i64::from(wcwidth(code));
        if !(1..=2).contains(&width) {
            return count;
        }
        let cols = self.cols;
        let (period, settled) = if self.dec.wraparound {
            let per_line = (cols / width).max(1);
            let lines = self.rows + to_i64(self.active().lines.max_length) + 2;
            (per_line, per_line.saturating_mul(lines))
        } else {
            (1, cols + 2)
        };
        if count <= settled {
            return count;
        }
        settled + (count - settled) % period
    }

    /// TS: setMode / resetMode (IRM, LNM)
    fn set_mode(&mut self, params: &Params, on: bool) {
        for index in 0..params.length {
            match params.param(index) {
                4 => self.insert_mode = on,
                20 => self.convert_eol = on,
                _ => {}
            }
        }
    }

    /// TS: InputHandler.setModePrivate (DECSET)
    fn set_mode_private(&mut self, params: &Params) {
        for index in 0..params.length {
            match params.param(index) {
                1 => self.dec.application_cursor_keys = true,
                2 => {
                    for level in 0..4 {
                        self.set_gcharset(level, None);
                    }
                }
                6 => {
                    self.dec.origin = true;
                    self.set_cursor(0, 0);
                }
                7 => self.dec.wraparound = true,
                25 => self.dec.cursor_hidden = false,
                45 => self.dec.reverse_wraparound = true,
                66 => self.dec.application_keypad = true,
                9 => self.mouse_protocol = MouseTrackingMode::X10,
                1000 => self.mouse_protocol = MouseTrackingMode::Vt200,
                1002 => self.mouse_protocol = MouseTrackingMode::Drag,
                1003 => self.mouse_protocol = MouseTrackingMode::Any,
                1004 => self.dec.send_focus = true,
                1048 => self.save_cursor(),
                1049 => {
                    self.save_cursor();
                    self.activate_alt_buffer();
                }
                47 | 1047 => self.activate_alt_buffer(),
                2004 => self.dec.bracketed_paste_mode = true,
                _ => {}
            }
        }
    }

    /// TS: InputHandler.resetModePrivate (DECRST)
    fn reset_mode_private(&mut self, params: &Params) {
        for index in 0..params.length {
            let mode = params.param(index);
            match mode {
                1 => self.dec.application_cursor_keys = false,
                6 => {
                    self.dec.origin = false;
                    self.set_cursor(0, 0);
                }
                7 => self.dec.wraparound = false,
                25 => self.dec.cursor_hidden = true,
                45 => self.dec.reverse_wraparound = false,
                66 => self.dec.application_keypad = false,
                9 | 1000 | 1002 | 1003 => self.mouse_protocol = MouseTrackingMode::None,
                1004 => self.dec.send_focus = false,
                1048 => self.restore_cursor(),
                1049 | 47 | 1047 => {
                    self.activate_normal_buffer();
                    if mode == 1049 {
                        self.restore_cursor();
                    }
                }
                2004 => self.dec.bracketed_paste_mode = false,
                _ => {}
            }
        }
    }

    /// TS: BufferSet.activateAltBuffer(eraseAttr)
    fn activate_alt_buffer(&mut self) {
        if self.alt_active {
            return;
        }
        let erase = self.erase_attr_data();
        let cols = self.cols;
        self.alt.fill_viewport_rows(cols, erase);
        self.alt.x = self.normal.x;
        self.alt.y = self.normal.y;
        self.alt_active = true;
    }

    /// TS: BufferSet.activateNormalBuffer — the alternate buffer is cleared on the way out.
    fn activate_normal_buffer(&mut self) {
        if !self.alt_active {
            return;
        }
        self.normal.x = self.alt.x;
        self.normal.y = self.alt.y;
        self.alt.clear();
        self.alt_active = false;
    }

    /// TS: InputHandler.softReset (DECSTR)
    fn soft_reset(&mut self) {
        let rows = self.rows;
        self.cur_attr = Attr::default();
        self.insert_mode = false;
        self.dec = DecModes::default();
        self.charset = None;
        self.charsets = [None; 4];
        self.glevel = 0;
        let cur = self.cur_attr;
        let buffer = self.buf();
        buffer.scroll_top = 0;
        buffer.scroll_bottom = rows - 1;
        buffer.saved_x = 0;
        buffer.saved_y = buffer.ybase;
        buffer.saved_attr.fg = cur.fg;
        buffer.saved_attr.bg = cur.bg;
        buffer.saved_charset = None;
        self.dec.origin = false;
    }

    /// TS: InputHandler.setScrollRegion (DECSTBM)
    fn set_scroll_region(&mut self, params: &Params) {
        let rows = self.rows;
        let top = params.param_or(0, 1);
        let mut bottom = params.param(1);
        if params.length < 2 || bottom > rows || bottom == 0 {
            bottom = rows;
        }
        if bottom > top {
            let buffer = self.buf();
            buffer.scroll_top = top - 1;
            buffer.scroll_bottom = bottom - 1;
            self.set_cursor(0, 0);
        }
    }

    // ── SGR ──────────────────────────────────────────────────────────────────────────────────────────────────────

    fn process_underline(attr: &mut Attr, style: i64) {
        let style = if style == -1 || style > 5 { 1 } else { style };
        let style = u32::try_from(style).unwrap_or(1);
        attr.extended.set_underline_style(style);
        attr.fg |= FG_UNDERLINE;
        if style == UNDERLINE_NONE {
            attr.fg &= !FG_UNDERLINE;
        }
        attr.update_extended();
    }

    fn process_sgr0(attr: &mut Attr) {
        attr.fg = 0;
        attr.bg = 0;
        attr.extended.set_underline_style(UNDERLINE_NONE);
        let color = attr.extended.underline_color() & !(CM_MASK | RGB_MASK);
        attr.extended.set_underline_color(color);
        attr.update_extended();
    }

    fn update_attr_color(color: u32, mode: i64, c1: i64, c2: i64, c3: i64) -> u32 {
        let mut color = color;
        if mode == 2 {
            color |= CM_RGB;
            color &= !RGB_MASK;
            color |= ((c1 as u32 & 255) << 16) | ((c2 as u32 & 255) << 8) | (c3 as u32 & 255);
        } else if mode == 5 {
            color &= !(CM_MASK | PCOLOR_MASK);
            color |= CM_P256 | (c1 as u32 & 0xff);
        }
        color
    }

    /// TS: InputHandler._extractColor — returns how many params were consumed beyond `pos`.
    fn extract_color(params: &Params, pos: usize, attr: &mut Attr) -> usize {
        let mut accu: [i64; 8] = [0, 0, -1, 0, 0, 0, 0, 0];
        let mut color_space = 0;
        let mut advance = 0;
        loop {
            if let Some(slot) = accu.get_mut(advance + color_space) {
                *slot = params.param(pos + advance);
            }
            if params.has_sub_params(pos + advance) {
                let sub_params = params.get_sub_params(pos + advance);
                let mut i = 0;
                loop {
                    if accu[1] == 5 {
                        color_space = 1;
                    }
                    if let Some(slot) = accu.get_mut(advance + i + 1 + color_space) {
                        *slot = sub_params.get(i).copied().map_or(0, i64::from);
                    }
                    i += 1;
                    if !(i < sub_params.len() && i + advance + 1 + color_space < 6) {
                        break;
                    }
                }
                break;
            }
            if (accu[1] == 5 && advance + color_space >= 2) || (accu[1] == 2 && advance + color_space >= 5) {
                break;
            }
            if accu[1] != 0 {
                color_space = 1;
            }
            advance += 1;
            if !(advance + pos < params.length && advance + color_space < 6) {
                break;
            }
        }
        for value in accu.iter_mut().take(6).skip(2) {
            if *value == -1 {
                *value = 0;
            }
        }
        match accu[0] {
            38 => attr.fg = Self::update_attr_color(attr.fg, accu[1], accu[3], accu[4], accu[5]),
            48 => attr.bg = Self::update_attr_color(attr.bg, accu[1], accu[3], accu[4], accu[5]),
            58 => {
                let color =
                    Self::update_attr_color(attr.extended.underline_color(), accu[1], accu[3], accu[4], accu[5]);
                attr.extended.set_underline_color(color);
            }
            _ => {}
        }
        advance
    }

    /// TS: InputHandler.charAttributes (SGR)
    fn char_attributes(&mut self, params: &Params) {
        let mut attr = self.cur_attr;
        if params.length == 1 && params.param(0) == 0 {
            Self::process_sgr0(&mut attr);
            self.cur_attr = attr;
            return;
        }
        let mut i = 0;
        while i < params.length {
            let p = params.param(i);
            match p {
                30..=37 => {
                    attr.fg &= !(CM_MASK | PCOLOR_MASK);
                    attr.fg |= CM_P16 | (p - 30) as u32;
                }
                40..=47 => {
                    attr.bg &= !(CM_MASK | PCOLOR_MASK);
                    attr.bg |= CM_P16 | (p - 40) as u32;
                }
                90..=97 => {
                    attr.fg &= !(CM_MASK | PCOLOR_MASK);
                    attr.fg |= CM_P16 | (p - 90) as u32 | 8;
                }
                100..=107 => {
                    attr.bg &= !(CM_MASK | PCOLOR_MASK);
                    attr.bg |= CM_P16 | (p - 100) as u32 | 8;
                }
                0 => Self::process_sgr0(&mut attr),
                1 => attr.fg |= FG_BOLD,
                3 => attr.bg |= BG_ITALIC,
                4 => {
                    attr.fg |= FG_UNDERLINE;
                    let style = if params.has_sub_params(i) {
                        params.get_sub_params(i).first().copied().map_or(0, i64::from)
                    } else {
                        i64::from(UNDERLINE_SINGLE)
                    };
                    Self::process_underline(&mut attr, style);
                }
                5 => attr.fg |= FG_BLINK,
                7 => attr.fg |= FG_INVERSE,
                8 => attr.fg |= FG_INVISIBLE,
                9 => attr.fg |= FG_STRIKETHROUGH,
                2 => attr.bg |= BG_DIM,
                21 => Self::process_underline(&mut attr, i64::from(UNDERLINE_DOUBLE)),
                22 => {
                    attr.fg &= !FG_BOLD;
                    attr.bg &= !BG_DIM;
                }
                23 => attr.bg &= !BG_ITALIC,
                24 => {
                    attr.fg &= !FG_UNDERLINE;
                    Self::process_underline(&mut attr, i64::from(UNDERLINE_NONE));
                }
                25 => attr.fg &= !FG_BLINK,
                27 => attr.fg &= !FG_INVERSE,
                28 => attr.fg &= !FG_INVISIBLE,
                29 => attr.fg &= !FG_STRIKETHROUGH,
                39 => attr.fg &= !(CM_MASK | RGB_MASK),
                49 => attr.bg &= !(CM_MASK | RGB_MASK),
                38 | 48 | 58 => i += Self::extract_color(params, i, &mut attr),
                53 => attr.bg |= BG_OVERLINE,
                55 => attr.bg &= !BG_OVERLINE,
                59 => {
                    attr.extended.set_underline_color(u32::MAX);
                    attr.update_extended();
                }
                _ => {}
            }
            i += 1;
        }
        self.cur_attr = attr;
    }
}

/// What moving the cursor down a line did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LineMove {
    /// It was on the scroll region's bottom line: the region scrolls (the caller does it).
    Scroll,
    /// It was on the screen's last line outside the region: it stays.
    Clamped,
    Moved,
}

/// The four column-editing sequences (SL, SR, DECIC, DECDC).
#[derive(Debug, Clone, Copy)]
enum ColumnOp {
    DeleteAtStart,
    InsertAtStart,
    InsertAtCursor,
    DeleteAtCursor,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn term(cols: u16, rows: u16) -> AlacrittyTerminal {
        AlacrittyTerminal::new(TerminalOptions {
            cols,
            rows,
            scrollback: 10_000,
        })
    }

    #[test]
    fn event_sink_keeps_the_last_title() {
        let sink = EventSink::default();
        assert_eq!(sink.title(), None);
        sink.send_event(Event::Title("one".into()));
        sink.send_event(Event::Wakeup);
        assert_eq!(sink.title().as_deref(), Some("one"));
        sink.send_event(Event::ResetTitle);
        assert_eq!(sink.title(), None);
    }

    #[test]
    fn dimensions_have_no_history() {
        let size = TermDimensions {
            columns: 80,
            screen_lines: 24,
        };
        assert_eq!((size.columns(), size.screen_lines(), size.total_lines()), (80, 24, 24));
    }

    #[test]
    fn cursor_parks_at_cols_after_the_last_column() {
        let mut t = term(5, 2);
        t.write_str("abcde");
        assert_eq!(t.cursor(), CursorPos { x: 5, y: 0 });
        t.write_str("f");
        assert_eq!(t.viewport_lines(true), ["abcde", "f"]);
        assert!(t.line_is_wrapped(1));
    }

    #[test]
    fn widths_follow_unicode_v6() {
        let mut t = term(20, 2);
        t.write_str("🎉日e\u{301}");
        assert_eq!(t.cursor().x, 4);
        assert_eq!(t.cell(0, 0).map(|cell| cell.width), Some(1));
        assert_eq!(t.cell(0, 1).map(|cell| cell.width), Some(2));
        assert_eq!(t.cell(0, 3).map(|cell| cell.chars), Some("e\u{301}".to_string()));
        assert_eq!(t.line_text(0, true).as_deref(), Some("🎉日e\u{301}"));
    }

    #[test]
    fn titles_and_modes() {
        let mut t = term(10, 3);
        t.write_str("\x1b]2;hello\x07\x1b[?1h\x1b[?2004h\x1b[?1002h\x1b[4h");
        assert_eq!(t.title().as_deref(), Some("hello"));
        let modes = t.modes();
        assert!(modes.application_cursor_keys_mode && modes.bracketed_paste_mode && modes.insert_mode);
        assert_eq!(modes.mouse_tracking_mode, MouseTrackingMode::Drag);
        t.write_str("\x1bc");
        assert_eq!(t.modes(), XtermModes::default());
        assert_eq!(t.title().as_deref(), Some("hello"));
    }

    #[test]
    fn alternate_screen_has_no_scrollback() {
        let mut t = term(10, 2);
        t.write_str("a\r\nb\r\nc");
        assert_eq!(t.base_y(), 1);
        t.write_str("\x1b[?1049h");
        assert_eq!(t.buffer_kind(), BufferKind::Alternate);
        assert_eq!(t.base_y(), 0);
        t.write_str("x\r\ny\r\nz");
        assert_eq!(t.base_y(), 0);
        t.write_str("\x1b[?1049l");
        assert_eq!(t.viewport_lines(true), ["b", "c"]);
    }

    #[test]
    fn sgr_colors_and_flags() {
        let mut t = term(10, 1);
        t.write_str("\x1b[1;31;48;5;200mA\x1b[0;38;2;1;2;3;4:3mB\x1b]8;;x\x1b\\C\x1b]8;;\x1b\\D");
        let a = t.cell(0, 0).map(|cell| cell.attrs);
        assert_eq!(
            a,
            Some(CellAttrs {
                fg: Color::P16(1),
                bg: Color::P256(200),
                flags: CellFlags::BOLD
            })
        );
        let b = t.cell(0, 1).map(|cell| cell.attrs);
        assert_eq!(b.map(|attrs| attrs.fg), Some(Color::Rgb(1, 2, 3)));
        assert_eq!(b.map(|attrs| attrs.flags), Some(CellFlags::UNDERLINE));
        assert!(
            t.cell(0, 2)
                .is_some_and(|cell| cell.attrs.flags.contains(CellFlags::UNDERLINE))
        );
        assert!(
            t.cell(0, 3)
                .is_some_and(|cell| cell.attrs.flags.contains(CellFlags::UNDERLINE))
        );
    }

    #[test]
    fn reflow_rejoins_wrapped_lines() {
        let mut t = term(4, 3);
        t.write_str("abcdef\r\n$ ");
        // Written spaces are content and survive trimming; erased cells do not.
        assert_eq!(t.viewport_lines(true), ["abcd", "ef", "$ "]);
        t.resize(8, 3);
        assert_eq!(t.viewport_lines(true), ["abcdef", "$ ", ""]);
        assert_eq!(t.cursor(), CursorPos { x: 2, y: 1 });
        t.resize(3, 3);
        assert_eq!(t.viewport_lines(true), ["abc", "def", "$ "]);
        assert_eq!(t.cursor(), CursorPos { x: 2, y: 2 });
        assert!(t.line_is_wrapped(1));
    }

    #[test]
    fn columns_never_drop_below_two() {
        let mut t = term(1, 1);
        assert_eq!((t.cols(), t.rows()), (2, 1));
        t.resize(0, 0);
        assert_eq!((t.cols(), t.rows()), (2, 1));
    }

    #[test]
    fn split_utf8_bytes_are_held_until_complete() {
        let mut t = term(10, 1);
        let bytes = "é".as_bytes();
        t.write(&bytes[..1]);
        t.write(&bytes[1..]);
        assert_eq!(t.line_text(0, true).as_deref(), Some("é"));
    }

    #[test]
    fn byte_order_marks_are_dropped_before_parsing() {
        // xterm's StringToUtf32/Utf8ToUtf32 decoders skip every U+FEFF, wherever it sits.
        let mut t = term(10, 2);
        t.write_str("a\u{FEFF}b\u{FEFF}");
        assert_eq!(t.line_text(0, true).as_deref(), Some("ab"));
        assert_eq!(t.cursor().x, 2);
        t.write_str("\r\n\u{FEFE}\u{FEFF}\u{FF00}");
        assert_eq!(t.line_text(1, true).as_deref(), Some("\u{FEFE}\u{FF00}"));
        let bytes = "\u{FEFF}c".as_bytes();
        t.write(&bytes[..2]);
        t.write(&bytes[2..]);
        assert_eq!(t.line_text(1, true).as_deref(), Some("\u{FEFE}\u{FF00}c"));
        t.write_str("\x1b]0;ti\u{FEFF}tle\x07");
        assert_eq!(t.title().as_deref(), Some("title"));
    }

    #[test]
    fn hostile_parameters_do_not_hang_or_panic() {
        let mut t = term(10, 3);
        t.write_str("x\x1b[2147483647b\x1b[2147483647L\x1b[2147483647I\x1b[99999999999999999999@");
        t.write_str("\x1b[1;1H\x1b[2147483647T\x1b[2147483647S\x1b[?2147483647h");
        t.write_str(&"\x1b[".repeat(100));
        t.write_str("\x1b]0;".repeat(10).as_str());
        assert_eq!(t.rows(), 3);
    }

    #[test]
    fn rep_with_nothing_to_repeat_returns_at_once() {
        // Without wraparound a wide character at the right margin is dropped, so REP finds an empty cell to repeat.
        let mut t = AlacrittyTerminal::new(TerminalOptions {
            cols: 80,
            rows: 24,
            scrollback: 1000,
        });
        let start = std::time::Instant::now();
        t.write_str("\x1b[?7l\x1b[1;80H日\x1b[2147483647bz");
        assert!(
            start.elapsed() < std::time::Duration::from_secs(2),
            "{:?}",
            start.elapsed()
        );
        assert_eq!(t.cursor(), CursorPos { x: 80, y: 0 });
        assert_eq!(
            t.line_text(0, true).as_deref(),
            Some(format!("{}z", " ".repeat(79)).as_str())
        );
        // A huge count of a printable character costs one pass over the buffer, not one print per repeat.
        let start = std::time::Instant::now();
        t.write_str("\x1b[?7h\x1b[H\x1b[2Ja\x1b[4194304b");
        assert!(
            start.elapsed() < std::time::Duration::from_secs(2),
            "{:?}",
            start.elapsed()
        );
        assert_eq!(t.cursor(), CursorPos { x: 65, y: 23 });
    }

    /// Everything the goldens can observe: every buffer line with its cells and wrap flag, cursor and scroll state.
    fn dump(t: &AlacrittyTerminal) -> String {
        let mut out = format!(
            "{:?} base {} viewport {} length {}\n",
            t.cursor(),
            t.base_y(),
            t.viewport_y(),
            t.length()
        );
        for y in 0..t.length() {
            out.push_str(&format!("{} {:?}", t.line_is_wrapped(y), t.line_text(y, false)));
            for x in 0..t.cols() {
                out.push_str(&format!(" {:?}", t.cell(y, x)));
            }
            out.push('\n');
        }
        out
    }

    #[test]
    fn rep_matches_printing_the_character_that_many_times() {
        let setups = [
            "",
            "\x1b[?7l",
            "\x1b[4h",
            "\x1b[?7l\x1b[4h",
            "\x1b[2;3r\x1b[2;1H",
            "\x1b[1;2r\x1b[1;1H",
            "\x1b[2;3r\x1b[4;1H",
            "\x1b[?1049h",
            "\x1b[?1049h\x1b[2;3r\x1b[2;1H",
            "\x1b[4h\x1b[1;3r\x1b[1;1H",
            "\x1b(0",
        ];
        for setup in setups {
            for ch in ["a", "日", "q"] {
                for count in (1..=160).chain([400, 401, 402, 403, 404, 1000, 1001]) {
                    let fresh = || {
                        let mut t = AlacrittyTerminal::new(TerminalOptions {
                            cols: 5,
                            rows: 4,
                            scrollback: 3,
                        });
                        t.write_str("\x1b[31mabcde\r\nfghij\r\nklmno\r\npqrst\x1b[H");
                        t.write_str(setup);
                        t.write_str(ch);
                        t
                    };
                    let mut repeated = fresh();
                    repeated.write_str(&format!("\x1b[{count}b"));
                    let mut printed = fresh();
                    printed.write_str(&ch.repeat(count));
                    assert_eq!(dump(&repeated), dump(&printed), "{setup:?} {ch:?} x{count}");
                }
            }
        }
    }
}
