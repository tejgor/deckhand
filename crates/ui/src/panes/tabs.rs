//! Port of tabs.tsx: the right pane's tab bar (Preview, Terminal, Git, Dev, Notes).

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use deckhand_protocol::data::RightPaneTab;

use crate::primitives::Row;

/// One tab of the bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TabSpec {
    pub key: RightPaneTab,
    pub label: &'static str,
    pub glyph: &'static str,
    pub hotkey: &'static str,
}

/// TS: tabs.tsx TABS.
pub const TABS: [TabSpec; 5] = [
    TabSpec {
        key: RightPaneTab::Preview,
        label: "Preview",
        glyph: "◈",
        hotkey: "p",
    },
    TabSpec {
        key: RightPaneTab::Terminal,
        label: "Terminal",
        glyph: "⌁",
        hotkey: "t",
    },
    TabSpec {
        key: RightPaneTab::Git,
        label: "Git",
        glyph: "⑂",
        hotkey: "g",
    },
    TabSpec {
        key: RightPaneTab::Dev,
        label: "Dev",
        glyph: "▹",
        hotkey: "d",
    },
    TabSpec {
        key: RightPaneTab::Notes,
        label: "Notes",
        glyph: "✎",
        hotkey: "a",
    },
];

/// TS: tabs.tsx TabBar as one row: `◈ p:Preview` per tab, two spaces apart; the active one bold and underlined, Dev
/// green with ` ●` while it runs.
pub fn tab_bar_row(active_tab: RightPaneTab, dev_running: bool) -> Row {
    let _ = (active_tab, dev_running);
    todo!("U1")
}

/// TS: tabs.tsx TabBar.
pub fn render_tab_bar(active_tab: RightPaneTab, dev_running: bool, buf: &mut Buffer, area: Rect) {
    let _ = (active_tab, dev_running, buf, area);
    todo!("U1")
}
