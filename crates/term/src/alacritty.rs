//! Port of HeadlessTerminal impl on alacritty_terminal.
//!
//! Stands in for `@xterm/headless` behind [`HeadlessTerminal`]; unit T1 implements it and gates it on the H0c
//! goldens (fixtures/term/*.golden.json).

use std::cell::RefCell;
use std::rc::Rc;

use alacritty_terminal::Term;
use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::vte::ansi::Processor;

use crate::headless::{BufferKind, CellInfo, CursorPos, HeadlessTerminal, TerminalOptions, XtermModes};

/// TS: @xterm/headless Terminal events deckhand observes (onTitleChange); also swallows replies (onData).
#[derive(Debug, Clone, Default)]
pub struct EventSink {
    /// The last title set by OSC 0/2 (`None` until one is set or after a reset).
    #[allow(dead_code)] // T1: read once the listener is implemented.
    title: Rc<RefCell<Option<String>>>,
}

impl EventListener for EventSink {
    /// TS: Terminal.onTitleChange
    fn send_event(&self, event: Event) {
        let _ = event;
        todo!("T1")
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
        todo!("T1")
    }

    fn screen_lines(&self) -> usize {
        todo!("T1")
    }

    fn columns(&self) -> usize {
        todo!("T1")
    }
}

/// TS: @xterm/headless Terminal, implemented on alacritty_terminal 0.25.
#[allow(dead_code)] // T1: fields are read once the impl below is written.
pub struct AlacrittyTerminal {
    term: Term<EventSink>,
    parser: Processor,
    events: EventSink,
    options: TerminalOptions,
}

impl HeadlessTerminal for AlacrittyTerminal {
    /// TS: new Terminal({cols, rows, scrollback, allowProposedApi: true})
    fn new(options: TerminalOptions) -> Self {
        let _ = options;
        todo!("T1")
    }

    /// TS: Terminal.write
    fn write(&mut self, data: &[u8]) {
        let _ = data;
        todo!("T1")
    }

    /// TS: Terminal.resize
    fn resize(&mut self, cols: u16, rows: u16) {
        let _ = (cols, rows);
        todo!("T1")
    }

    /// TS: Terminal.cols
    fn cols(&self) -> u16 {
        todo!("T1")
    }

    /// TS: Terminal.rows
    fn rows(&self) -> u16 {
        todo!("T1")
    }

    /// TS: Terminal.buffer.active.type
    fn buffer_kind(&self) -> BufferKind {
        todo!("T1")
    }

    /// TS: Terminal.buffer.active.baseY
    fn base_y(&self) -> usize {
        todo!("T1")
    }

    /// TS: Terminal.buffer.active.length
    fn length(&self) -> usize {
        todo!("T1")
    }

    /// TS: IBuffer.getLine(y)?.translateToString(trimRight)
    fn line_text(&self, y: usize, trim_right: bool) -> Option<String> {
        let _ = (y, trim_right);
        todo!("T1")
    }

    /// TS: IBuffer.getLine(y)?.isWrapped
    fn line_is_wrapped(&self, y: usize) -> bool {
        let _ = y;
        todo!("T1")
    }

    /// TS: IBuffer.getLine(y)?.getCell(x)
    fn cell(&self, y: usize, x: u16) -> Option<CellInfo> {
        let _ = (y, x);
        todo!("T1")
    }

    /// TS: IBuffer.cursorX / cursorY
    fn cursor(&self) -> CursorPos {
        todo!("T1")
    }

    /// TS: Terminal.modes
    fn modes(&self) -> XtermModes {
        todo!("T1")
    }

    /// TS: Terminal.onTitleChange (last value)
    fn title(&self) -> Option<String> {
        todo!("T1")
    }
}
