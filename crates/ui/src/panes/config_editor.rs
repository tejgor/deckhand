//! Port of configEditorPane.tsx: the raw JSON editor of a config layer (E in Settings): one row per editor line, the
//! cursor inverse, scrolled only as far as needed to keep it on screen.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use deckhand_core::text_editor::EditorState;
use deckhand_protocol::data::ProjectConfigDocument;

/// TS: configEditorPane.tsx visible: CR and tab as spaces, other controls and line separators `?`.
pub fn visible(text: &str) -> String {
    let _ = text;
    todo!("U5")
}

/// TS: configEditorPane.tsx scrollTopFor: scroll only as far as needed to keep the cursor row on screen.
pub fn scroll_top_for(previous: usize, cursor_row: usize, rows: usize, line_count: usize) -> usize {
    let _ = (previous, cursor_row, rows, line_count);
    todo!("U5")
}

/// TS: configEditorPane.tsx ConfigEditorPane props (width and height are the area's).
#[derive(Clone, Copy, Debug)]
pub struct ConfigEditorPaneProps<'a> {
    pub document: &'a ProjectConfigDocument,
    pub state: &'a EditorState,
    pub error: Option<&'a str>,
}

/// TS: configEditorPane.tsx ConfigEditorPane (`scroll_top`: its `useRef` scroll position, kept between renders).
pub fn render_config_editor_pane(
    props: &ConfigEditorPaneProps<'_>,
    scroll_top: &mut usize,
    buf: &mut Buffer,
    area: Rect,
) {
    let _ = (props, scroll_top, buf, area);
    todo!("U5")
}
