//! The right pane's tab bar (Preview, Terminal, Git, Dev, Notes).

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use deckhand_core::tasks::open_note_items;
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
    /// Not in the TS: unchecked items in the notes the Notes tab shows (` ☐ N`).
    pub open_notes: usize,
}

/// The unchecked checklist items of the notes `session`'s Notes tab shows: its own and the shared one (a deleted
/// worktree's read-only note left out).
pub fn open_note_count(session: &SessionRecord) -> usize {
    let shared = session
        .shared_notes
        .as_ref()
        .filter(|note| note.read_only != Some(true))
        .map(|note| note.text.as_str());
    open_note_items(session.notes.as_deref()).len() + open_note_items(shared).len()
}

/// TS: tabs.tsx TabBar as one row: `◈ p:Preview` per tab, two spaces apart; the active one bold and underlined, Dev
/// green with ` ●` while it runs; not in the TS: Notes with ` ☐ N` while its notes have open items.
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
            RightPaneTab::Notes if badges.open_notes > 0 => format!(" ☐ {}", badges.open_notes),
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
            open_notes: 0,
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
    fn notes_counts_the_open_items_of_both_notes_it_shows() {
        let session: SessionRecord = serde_json::from_value(serde_json::json!({
            "id": "a", "title": "a", "program": "claude", "command": "claude", "cwd": "/r", "repoRoot": "/r",
            "status": "running", "createdAt": "", "updatedAt": "",
            "notes": "- [ ] mine\n- [x] done",
            "sharedNotes": {"kind": "worktree", "id": "w", "path": "/n/w.md", "revision": "r", "text": "- [ ] ours"}
        }))
        .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(open_note_count(&session), 2);
        let badges = TabBadges {
            dev_running: false,
            open_notes: open_note_count(&session),
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
