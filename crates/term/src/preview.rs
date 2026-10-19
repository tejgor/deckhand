//! Port of terminalPreview.ts.
//!
//! The TS class chained every write/resize on a promise because xterm parses asynchronously; [`HeadlessTerminal`]
//! parses synchronously, so these methods are plain `&mut self` calls (no await, nothing to hold across one) and a
//! read always sees every earlier write. Sizes and scroll offsets stay JS numbers (`f64`) so NaN/±Infinity,
//! fractions and negatives normalise exactly as in TS (the H0c goldens exercise 2.5, -3 and 1e6).

use serde::{Deserialize, Serialize};

use crate::alacritty::AlacrittyTerminal;
use crate::headless::HeadlessTerminal;

/// TS: terminalPreview.ts DEFAULT_SCROLLBACK
pub const DEFAULT_SCROLLBACK: usize = 10_000;

/// TS: terminalPreview.ts clampSize — `fallback` when not finite, else `max(1, floor(value))`.
pub fn clamp_size(value: f64, fallback: u16) -> u16 {
    let _ = (value, fallback);
    todo!("T1")
}

/// TS: terminalPreview.ts stripTrailingEmptyLines
pub fn strip_trailing_empty_lines(lines: Vec<String>) -> Vec<String> {
    let _ = lines;
    todo!("T1")
}

/// TS: terminalPreview.ts TerminalPreview.getScrollInfo result `{scrollOffset, maxScrollOffset}`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScrollInfo {
    pub scroll_offset: usize,
    pub max_scroll_offset: usize,
}

/// TS: terminalPreview.ts TerminalPreview
#[allow(dead_code)] // T1: fields are read once the methods below are written.
pub struct TerminalPreview<T: HeadlessTerminal = AlacrittyTerminal> {
    terminal: T,
    snapshot: String,
    snapshot_scroll_offset: usize,
    dirty: bool,
    cols: u16,
    rows: u16,
}

impl<T: HeadlessTerminal> TerminalPreview<T> {
    /// TS: new TerminalPreview(cols, rows) — sizes clamped with fallbacks 80×24, scrollback [`DEFAULT_SCROLLBACK`].
    pub fn new(cols: f64, rows: f64) -> Self {
        let _ = (cols, rows);
        todo!("T1")
    }

    /// TS: TerminalPreview.write(data) — feeds decoded PTY output and marks the snapshot dirty.
    pub fn write(&mut self, data: &str) {
        let _ = data;
        todo!("T1")
    }

    /// TS: TerminalPreview.resize(cols, rows) — no-op when the clamped size is unchanged.
    pub fn resize(&mut self, cols: f64, rows: f64) {
        let _ = (cols, rows);
        todo!("T1")
    }

    /// TS: TerminalPreview.getSnapshot(scrollOffset = 0) — viewport text at a scroll offset, cached.
    pub fn get_snapshot(&mut self, scroll_offset: Option<f64>) -> String {
        let _ = scroll_offset;
        todo!("T1")
    }

    /// TS: TerminalPreview.getScrollInfo(scrollOffset = 0)
    pub fn get_scroll_info(&self, scroll_offset: Option<f64>) -> ScrollInfo {
        let _ = scroll_offset;
        todo!("T1")
    }

    /// TS: TerminalPreview.getAnsiFrame — clear + per-row cursor moves + text + final cursor position.
    pub fn get_ansi_frame(&mut self) -> String {
        todo!("T1")
    }

    /// TS: TerminalPreview.getCachedSnapshot — the last computed snapshot without refreshing.
    pub fn get_cached_snapshot(&self) -> String {
        todo!("T1")
    }

    /// TS: TerminalPreview.dispose
    pub fn dispose(self) {
        todo!("T1")
    }

    /// The preview's own terminal (term.gen.ts reads `preview.terminal` for the golden screen snapshot).
    pub fn terminal(&self) -> &T {
        todo!("T1")
    }

    /// TS: TerminalPreview.refreshSnapshot(scrollOffset = 0)
    #[allow(dead_code)] // T1: called by the public methods once implemented.
    fn refresh_snapshot(&mut self, scroll_offset: f64) {
        let _ = scroll_offset;
        todo!("T1")
    }

    /// TS: TerminalPreview.getMaxScrollOffset — `max(0, buffer.active.baseY)`.
    #[allow(dead_code)] // T1: called by the public methods once implemented.
    fn get_max_scroll_offset(&self) -> usize {
        todo!("T1")
    }

    /// TS: TerminalPreview.normalizeScrollOffset — 0 when not finite, else floor clamped to `[0, max]`.
    #[allow(dead_code)] // T1: called by the public methods once implemented.
    fn normalize_scroll_offset(&self, scroll_offset: f64) -> usize {
        let _ = scroll_offset;
        todo!("T1")
    }
}
