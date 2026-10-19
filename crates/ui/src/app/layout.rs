//! Port of app.tsx's geometry and frame: the sidebar width rules, the `layout` memo (pane sizes), the header (title,
//! task count, update hint, connection), the right pane per mode and the two-row footer (key hint and messages).

use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;

use deckhand_core::display::HintPart;
use deckhand_protocol::data::{RightPaneTab, SessionRecord};

use crate::app::state::{App, Mode, TerminalSize};
use crate::primitives::Row;

/// TS: app.tsx HEADER_ROWS.
pub const HEADER_ROWS: u16 = 2;

/// TS: app.tsx FOOTER_ROWS: the key hint and one combined message line, always.
pub const FOOTER_ROWS: u16 = 2;

/// TS: app.tsx SPARE_ROWS: one spare row below the footer (Ink repainted the whole screen once output reached the
/// terminal height).
pub const SPARE_ROWS: u16 = 1;

/// TS: app.tsx sidebarWidth: 24% of the width, 24..34, clamped.
pub fn sidebar_width(total_width: i64) -> i64 {
    let _ = total_width;
    todo!("A1")
}

/// TS: app.tsx clampSidebarWidth.
pub fn clamp_sidebar_width(width: i64, total_width: i64) -> i64 {
    let _ = (width, total_width);
    todo!("A1")
}

/// TS: app.tsx `layout` memo.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AppLayout {
    pub sidebar_width: u16,
    pub preview_width: u16,
    pub content_height: u16,
    /// The right pane minus border and padding (4 columns).
    pub pane_inner_width: u16,
    /// The right pane minus border, tab bar and spacer (4 rows).
    pub pane_inner_height: u16,
    pub preview_cols: u16,
    pub preview_rows: u16,
}

/// TS: app.tsx `layout`: pane sizes for the terminal size and the sidebar override.
pub fn app_layout(size: TerminalSize, sidebar_width_override: Option<i64>) -> AppLayout {
    let _ = (size, sidebar_width_override);
    todo!("A1")
}

/// TS: footerHint's `terminalAction` argument.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TerminalActionHint {
    pub switch_hint: String,
    pub finished: bool,
}

/// TS: app.tsx footerHint's arguments.
#[derive(Clone, Copy, Debug)]
pub struct FooterHintInput<'a> {
    pub mode: Mode,
    pub active_tab: RightPaneTab,
    pub width: usize,
    pub session: Option<&'a SessionRecord>,
    pub scroll_sensitivity: f64,
    pub attach_ready: bool,
    pub notes_hint: Option<&'a str>,
    pub terminal_action: Option<&'a TerminalActionHint>,
}

/// TS: app.tsx footerHint: the footer's key hint per mode (empty where the screen shows its own).
pub fn footer_hint(input: &FooterHintInput<'_>) -> String {
    let _ = input;
    todo!("A1")
}

/// TS: app.tsx FooterMessage.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FooterMessage {
    pub text: String,
    pub color: Color,
}

/// TS: app.tsx candidateMessages: error, busy, numeric selection, status and the selected session's cleanup error.
pub fn footer_messages(app: &App) -> Vec<FooterMessage> {
    let _ = app;
    todo!("A1")
}

/// TS: app.tsx footerRows: exactly two rows, each truncated, so the layout above never shifts.
pub fn footer_rows(app: &App) -> [Row; 2] {
    let _ = app;
    todo!("A1")
}

/// TS: app.tsx the header: the title row (`deckhand` / DEV, task count, update hint, connection) and the repo path.
pub fn header_rows(app: &App) -> [Row; 2] {
    let _ = app;
    todo!("A1")
}

/// TS: app.tsx the right pane in the tabbed modes: the bordered box with the tab bar and the active tab.
pub fn render_tabbed_pane(app: &mut App, buf: &mut Buffer, area: Rect) {
    let _ = (app, buf, area);
    todo!("A1")
}

/// TS: app.tsx the content row per mode: full-width screens (settings, help, agents), or the sidebar beside the
/// mode's right pane.
pub fn render_content(app: &mut App, buf: &mut Buffer, area: Rect) {
    let _ = (app, buf, area);
    todo!("A1")
}

/// TS: app.tsx App's render: header, content, footer.
pub fn render_app(app: &mut App, frame: &mut Frame<'_>) {
    let _ = (app, frame);
    todo!("A1")
}

/// The hint parts of the browse footer (TS footerHint's `browse` case), before fitting.
pub fn browse_hint_parts(input: &FooterHintInput<'_>) -> Vec<HintPart> {
    let _ = input;
    todo!("A1")
}
