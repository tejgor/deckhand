//! The right pane's tab bar (Preview, Terminal, Git, Dev, Notes).

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use deckhand_core::task_list::TaskList;
use deckhand_protocol::data::{RightPaneTab, SessionRecord};

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

/// What the tab bar marks beside the tab names.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TabBadges {
    /// The workspace's Dev runs: Dev is green with ` ●`.
    pub dev_running: bool,
    /// Not in the TS: the open steps the Notes tab shows (` ☐ N`).
    pub open_steps: usize,
}

/// The open steps of `session`: the unticked lines under its line (notes' checkboxes are text, not counted).
pub fn open_step_count(list: &TaskList, session: &SessionRecord) -> usize {
    list.line_of_session(&session.id).map_or(0, |line| {
        list.children(&line.id).iter().filter(|step| !step.done).count()
    })
}

/// TS: tabs.tsx TabBar as one row: `◈ p:Preview` per tab, two spaces apart; the active one bold and underlined, Dev
/// green with ` ●` while it runs; not in the TS: Notes with ` ☐ N` while the session has open steps.
pub fn tab_bar_row(active_tab: RightPaneTab, badges: TabBadges) -> Row {
    let mut row = Row::new();
    for (index, tab) in TABS.iter().enumerate() {
        let active = tab.key == active_tab;
        let running = tab.key == RightPaneTab::Dev && badges.dev_running;
        let color = if active {
            THEME.active
        } else if running {
            THEME.success
        } else {
            THEME.muted
        };
        let badge = match tab.key {
            RightPaneTab::Dev if running => " ●".to_owned(),
            RightPaneTab::Notes if badges.open_steps > 0 => format!(" ☐ {}", badges.open_steps),
            _ => String::new(),
        };
        let style = TextStyle::fg(color).bold(active).underline(active);
        row.push(Seg::new(
            format!("{} {}:{}{badge}", tab.glyph, tab.hotkey, tab.label),
            style,
        ));
        if index + 1 < TABS.len() {
            row.push(Seg::plain("  "));
        }
    }
    row
}

/// TS: tabs.tsx TabBar.
pub fn render_tab_bar(active_tab: RightPaneTab, badges: TabBadges, buf: &mut Buffer, area: Rect) {
    draw_rows(buf, area, &[tab_bar_row(active_tab, badges)]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::row_text;
    use ratatui::style::Color;

    #[test]
    fn lists_every_tab_two_spaces_apart() {
        let row = tab_bar_row(RightPaneTab::Preview, TabBadges::default());
        assert_eq!(row_text(&row), "◈ p:Preview  ⌁ t:Terminal  ⑂ g:Git  ▹ d:Dev  ✎ a:Notes");
        assert_eq!(row[0].style.color, Some(Color::Cyan));
        assert_eq!(row[0].style.underline, Some(true));
        assert_eq!(row[2].style.color, Some(Color::DarkGray));
    }

    #[test]
    fn a_running_dev_tab_is_green_with_a_dot() {
        let running = TabBadges {
            dev_running: true,
            open_steps: 0,
        };
        let row = tab_bar_row(RightPaneTab::Notes, running);
        assert!(row_text(&row).contains("▹ d:Dev ●"));
        let dev = row
            .iter()
            .find(|seg| seg.text.contains("d:Dev"))
            .map(|seg| seg.style.color);
        assert_eq!(dev, Some(Some(Color::Green)));
        let active = tab_bar_row(RightPaneTab::Dev, running);
        let dev = active
            .iter()
            .find(|seg| seg.text.contains("d:Dev"))
            .map(|seg| seg.style.color);
        assert_eq!(dev, Some(Some(Color::Cyan)));
    }

    #[test]
    fn notes_counts_the_open_steps_of_the_sessions_line() {
        let session: SessionRecord = serde_json::from_value(serde_json::json!({
            "id": "a", "title": "a", "program": "claude", "command": "claude", "cwd": "/r", "repoRoot": "/r",
            "status": "running", "createdAt": "", "updatedAt": "",
            "notes": "- [ ] a checkbox in a note is not a step"
        }))
        .unwrap_or_else(|error| panic!("{error}"));
        let list = TaskList::parse(
            "- [ ] Auth <!-- dh:t=l1 s=a -->\n  - [ ] one <!-- dh:t=l2 -->\n  - [x] two <!-- dh:t=l3 -->\n  \
             - [ ] three <!-- dh:t=l4 -->\n    - [ ] not direct <!-- dh:t=l5 -->\n",
        );
        assert_eq!(open_step_count(&list, &session), 2);
        assert_eq!(open_step_count(&TaskList::default(), &session), 0);
        let badges = TabBadges {
            dev_running: false,
            open_steps: open_step_count(&list, &session),
        };
        assert!(row_text(&tab_bar_row(RightPaneTab::Preview, badges)).ends_with("✎ a:Notes ☐ 2"));
        assert!(row_text(&tab_bar_row(RightPaneTab::Preview, TabBadges::default())).ends_with("✎ a:Notes"));
    }

    #[test]
    fn renders_into_the_buffer() {
        let area = Rect::new(0, 0, 60, 1);
        let mut buf = Buffer::empty(area);
        render_tab_bar(RightPaneTab::Git, TabBadges::default(), &mut buf, area);
        let line: String = (0..area.width).map(|x| buf[(x, 0)].symbol().to_string()).collect();
        assert!(line.starts_with("◈ p:Preview  ⌁ t:Terminal"));
    }
}
