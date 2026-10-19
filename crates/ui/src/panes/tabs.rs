//! Port of tabs.tsx: the right pane's tab bar (Preview, Terminal, Git, Dev, Notes).

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use deckhand_protocol::data::RightPaneTab;

use crate::primitives::{Row, Seg, TextStyle, draw_rows};
use crate::theme::THEME;

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
    let mut row = Row::new();
    for (index, tab) in TABS.iter().enumerate() {
        let active = tab.key == active_tab;
        let running = tab.key == RightPaneTab::Dev && dev_running;
        let color = if active {
            THEME.active
        } else if running {
            THEME.success
        } else {
            THEME.muted
        };
        let dot = if running { " ●" } else { "" };
        let style = TextStyle::fg(color).bold(active).underline(active);
        row.push(Seg::new(
            format!("{} {}:{}{dot}", tab.glyph, tab.hotkey, tab.label),
            style,
        ));
        if index + 1 < TABS.len() {
            row.push(Seg::plain("  "));
        }
    }
    row
}

/// TS: tabs.tsx TabBar.
pub fn render_tab_bar(active_tab: RightPaneTab, dev_running: bool, buf: &mut Buffer, area: Rect) {
    draw_rows(buf, area, &[tab_bar_row(active_tab, dev_running)]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::row_text;
    use ratatui::style::Color;

    #[test]
    fn lists_every_tab_two_spaces_apart() {
        let row = tab_bar_row(RightPaneTab::Preview, false);
        assert_eq!(row_text(&row), "◈ p:Preview  ⌁ t:Terminal  ⑂ g:Git  ▹ d:Dev  ✎ a:Notes");
        assert_eq!(row[0].style.color, Some(Color::Cyan));
        assert_eq!(row[0].style.underline, Some(true));
        assert_eq!(row[2].style.color, Some(Color::DarkGray));
    }

    #[test]
    fn a_running_dev_tab_is_green_with_a_dot() {
        let row = tab_bar_row(RightPaneTab::Notes, true);
        assert!(row_text(&row).contains("▹ d:Dev ●"));
        let dev = row
            .iter()
            .find(|seg| seg.text.contains("d:Dev"))
            .map(|seg| seg.style.color);
        assert_eq!(dev, Some(Some(Color::Green)));
        let active = tab_bar_row(RightPaneTab::Dev, true);
        let dev = active
            .iter()
            .find(|seg| seg.text.contains("d:Dev"))
            .map(|seg| seg.style.color);
        assert_eq!(dev, Some(Some(Color::Cyan)));
    }

    #[test]
    fn renders_into_the_buffer() {
        let area = Rect::new(0, 0, 60, 1);
        let mut buf = Buffer::empty(area);
        render_tab_bar(RightPaneTab::Git, false, &mut buf, area);
        let line: String = (0..area.width).map(|x| buf[(x, 0)].symbol().to_string()).collect();
        assert!(line.starts_with("◈ p:Preview  ⌁ t:Terminal"));
    }
}
