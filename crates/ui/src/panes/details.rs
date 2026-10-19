//! Port of detailsPane.tsx: a scrollable text pane with one hint line (the trust review, the workspace overview,
//! the DELETE override, the discard prompt). Text is shown exactly: tabs as four spaces, controls and invisible
//! characters (bidi overrides, zero-width) escaped as `<U+XXXX>`, rows wrapped by code points.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use deckhand_core::display::HintPart;
use deckhand_core::key::Input;

/// TS: detailsPane.tsx DETAILS_CHROME_ROWS: border (2) + title + hint rows around the body.
pub const DETAILS_CHROME_ROWS: usize = 4;

/// TS: detailsPane.tsx DETAILS_CHROME_COLUMNS: border (2) + paddingX (2).
pub const DETAILS_CHROME_COLUMNS: usize = 4;

/// TS: detailsPane.tsx INVISIBLE_PATTERN's characters (bidi controls, zero-width, line separators, BOM).
pub fn is_invisible(c: char) -> bool {
    let _ = c;
    todo!("U3")
}

/// TS: detailsPane.tsx codePointEscape (`<U+200B>`).
pub fn code_point_escape(c: char) -> String {
    let _ = c;
    todo!("U3")
}

/// TS: detailsPane.tsx visibleDetailText: never silently drop characters (details panes include the trust review).
pub fn visible_detail_text(text: &str) -> String {
    let _ = text;
    todo!("U3")
}

/// TS: detailsPane.tsx detailLines: wrapped by code points at `width` so surrogate pairs are never split; an empty
/// line is one space.
pub fn detail_lines(text: &str, width: usize) -> Vec<String> {
    let _ = (text, width);
    todo!("U3")
}

/// TS: detailsPane.tsx DetailsViewport.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DetailsViewport {
    pub lines: Vec<String>,
    pub rows: usize,
    pub max: usize,
}

/// TS: detailsPane.tsx detailsViewport (the pane's `width` × `height`, chrome included).
pub fn details_viewport(text: &str, width: usize, height: usize) -> DetailsViewport {
    let _ = (text, width, height);
    todo!("U3")
}

/// TS: detailsPane.tsx clampScroll.
pub fn clamp_scroll(scroll: usize, max: usize) -> usize {
    let _ = (scroll, max);
    todo!("U3")
}

/// TS: detailsPane.tsx scrollDetails: ↑↓ j k, PgUp/PgDn, Home/End; `None` when the key does not scroll (callers
/// handle it). `rows` and `max` are the viewport's.
pub fn scroll_details(scroll: usize, input: &Input, rows: usize, max: usize) -> Option<usize> {
    let _ = (scroll, input, rows, max);
    todo!("U3")
}

/// TS: detailsPane.tsx DetailsPane props (`footer`: a plain string is one part).
#[derive(Clone, Copy, Debug)]
pub struct DetailsPaneProps<'a> {
    pub title: &'a str,
    pub text: &'a str,
    pub footer: &'a [HintPart],
    pub scroll: usize,
}

/// TS: detailsPane.tsx DetailsPane.
pub fn render_details_pane(props: &DetailsPaneProps<'_>, buf: &mut Buffer, area: Rect) {
    let _ = (props, buf, area);
    todo!("U3")
}
