//! Port of helpPane.tsx: the ? guide (a topic list beside the selected topic, each a table of key → what it does
//! with a few notes; / searches every topic; narrow panes show "◂ topic ▸" in the title instead of the list).

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use deckhand_core::display::DetailPart;
use deckhand_core::help::{HelpLine, HelpTopic};
use deckhand_core::key::Input;

/// TS: helpPane.tsx LIST_WIDTH: from this inner width the topic list is shown beside the content.
pub const LIST_WIDTH: usize = 64;

/// TS: helpPane.tsx MAX_KEY_WIDTH: a longer key gets its own row instead of widening the key column.
pub const MAX_KEY_WIDTH: usize = 15;

/// TS: helpPane.tsx Row: one rendered line as styled parts.
pub type HelpRow = Vec<DetailPart>;

/// A topic with only the lines that matched a search (TS `HelpTopic` objects built by searchHelp).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FoundTopic {
    pub title: &'static str,
    pub lines: Vec<HelpLine>,
}

/// TS: helpPane.tsx searchHelp: the topics with their lines matching `query` (case-insensitive), in order; headings
/// are dropped. `topics` defaults to HELP_TOPICS in TS.
pub fn search_help(query: &str, topics: &[HelpTopic]) -> Vec<FoundTopic> {
    let _ = (query, topics);
    todo!("U2")
}

/// TS: helpPane.tsx helpRows: a topic's lines at `width` (aligned key column, wrapped descriptions and notes).
pub fn help_rows(lines: &[HelpLine], width: usize) -> Vec<HelpRow> {
    let _ = (lines, width);
    todo!("U2")
}

/// TS: helpPane.tsx searchRows: each matching topic's numbered title, then its matching lines.
pub fn search_rows(query: &str, width: usize) -> Vec<HelpRow> {
    let _ = (query, width);
    todo!("U2")
}

/// TS: helpPane.tsx HelpPane's `onLayout`: the scroll range and page size of the last render.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HelpViewport {
    pub max: usize,
    pub page: usize,
}

impl Default for HelpViewport {
    fn default() -> Self {
        HelpViewport { max: 0, page: 1 }
    }
}

/// What a key did to the guide (TS handleInput's `'close'`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HelpOutcome {
    Close,
}

/// TS: helpPane.tsx HelpView / useHelp: the guide's state.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HelpView {
    pub topic: usize,
    pub scroll: usize,
    /// `None`: browsing topics; `Some`: search results (typed while `typing`).
    pub query: Option<String>,
    pub typing: bool,
    /// The last render's scroll range and page size, so keys can clamp.
    pub view: HelpViewport,
}

impl HelpView {
    /// TS: HelpView.open: on the first topic (or `topic`), not searching.
    pub fn open(&mut self, topic: Option<usize>) {
        let _ = topic;
        todo!("U2")
    }

    /// TS: useHelp pick: topic `index`, wrapping around.
    pub fn pick(&mut self, index: isize) {
        let _ = index;
        todo!("U2")
    }

    /// TS: useHelp scrollBy.
    pub fn scroll_by(&mut self, delta: isize) {
        let _ = delta;
        todo!("U2")
    }

    /// TS: HelpView.handleInput: keys while the guide is open; `Close` when it should close.
    pub fn handle_input(&mut self, input: &Input) -> Option<HelpOutcome> {
        let _ = input;
        todo!("U2")
    }

    /// TS: HelpView.render: draws the guide and records the viewport for the next key.
    pub fn render(&mut self, buf: &mut Buffer, area: Rect) {
        let _ = (buf, area);
        todo!("U2")
    }
}

/// TS: helpPane.tsx HelpPane props (width and height are the area's).
#[derive(Clone, Copy, Debug)]
pub struct HelpPaneProps<'a> {
    pub topic: usize,
    pub scroll: usize,
    pub query: Option<&'a str>,
    pub typing: bool,
}

/// TS: helpPane.tsx HelpPane: draws the guide; returns what `onLayout` reported.
pub fn render_help_pane(props: &HelpPaneProps<'_>, buf: &mut Buffer, area: Rect) -> HelpViewport {
    let _ = (props, buf, area);
    todo!("U2")
}
