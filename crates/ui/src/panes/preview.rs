//! The Preview tab (the agent's screen, or what a session that is not running leaves: setup output, its last preview, a
//! hint). While you type to the agent (preview focus) it draws the agent's screen in its own colours.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color as TermColor, Modifier, Style};

use deckhand_core::display::{compact_path, fit_lines, plain_terminal_text, status_label, truncate};
use deckhand_core::jsstr::{js_len, js_trim};
use deckhand_protocol::data::{
    PreviewRecord, SessionRecord, SessionSetup, SessionStatus, SessionWorktreeMode, SetupState,
};
use deckhand_protocol::screen::{CellAttrs, CellFlags, Color, PreviewScreen};

use crate::primitives::{Row, Seg, TextStyle, Wrap, draw_rows, fit_row, row_width, space_between, truncate_row};
use crate::theme::THEME;

/// TS: preview.tsx PreviewPaneProps (width and height are the area's).
#[derive(Clone, Copy, Debug)]
pub struct PreviewPaneProps<'a> {
    pub session: Option<&'a SessionRecord>,
    pub preview: &'a PreviewRecord,
    pub spinner_frame: &'a str,
    /// Preview focus: you type to the agent, and its styled screen is drawn once it arrives.
    pub focused: bool,
}

/// TS: preview.tsx setupMessage: the setup's header line and the tail of its output in `rows` rows, as plain text.
pub fn setup_message(setup: &SessionSetup, rows: usize) -> String {
    let output = plain_terminal_text(&setup.output);
    let output = output.trim_end_matches('\n');
    let tail: Vec<&str> = if !output.is_empty() && rows > 1 {
        let lines: Vec<&str> = output.split('\n').collect();
        lines[lines.len().saturating_sub(rows - 1)..].to_vec()
    } else {
        Vec::new()
    };
    let header = plain_terminal_text(&format!("Setup ({}): {}", setup.state, setup.command)).replace('\n', " ");
    std::iter::once(header.as_str())
        .chain(tail)
        .collect::<Vec<_>>()
        .join("\n")
}

/// TS: preview.tsx fallbackMessage: what the body shows (the preview, or why there is none).
pub fn fallback_message(session: Option<&SessionRecord>, preview: &PreviewRecord, rows: usize) -> String {
    let Some(session) = session else {
        return "No sessions yet. Press n to create one.".to_owned();
    };
    if session.status == SessionStatus::Starting {
        return match &session.setup {
            Some(setup) => setup_message(setup, rows),
            None => "Starting session…".to_owned(),
        };
    }
    if session.status == SessionStatus::Exited
        && let Some(setup) = &session.setup
        && matches!(setup.state, SetupState::Failed | SetupState::Cancelled)
    {
        // Setup runs before the agent, so its output is the relevant context here.
        let note = if preview.content.starts_with("Setup (") {
            String::new()
        } else {
            let plain = plain_terminal_text(&preview.content);
            js_trim(&plain).split('\n').take(2).collect::<Vec<_>>().join("\n")
        };
        return if note.is_empty() {
            setup_message(setup, rows)
        } else {
            let used = note.split('\n').count();
            format!("{note}\n{}", setup_message(setup, rows.saturating_sub(used)))
        };
    }
    let content = preview.content.as_str();
    if session.status == SessionStatus::Exited {
        return if content.is_empty() { "Session exited." } else { content }.to_owned();
    }
    if content.is_empty() {
        "Waiting for agent output…".to_owned()
    } else {
        content.to_owned()
    }
}

/// TS: preview.tsx worktreeBadge (`worktree` / `attached`).
pub fn worktree_badge(session: &SessionRecord) -> Option<&'static str> {
    match session.worktree.as_ref()?.mode {
        SessionWorktreeMode::None => None,
        SessionWorktreeMode::Managed => Some("worktree"),
        SessionWorktreeMode::Attached => Some("attached"),
    }
}

/// TS: preview.tsx PreviewPane's header row at `width`: the session's path, then its badges.
fn header_row(props: &PreviewPaneProps<'_>, width: usize) -> Row {
    let PreviewPaneProps {
        session,
        preview,
        focused,
        ..
    } = *props;
    let scrolled = preview.scroll_offset.filter(|offset| *offset != 0);
    let mut badges: Vec<String> = Vec::new();
    if let Some(session) = session {
        badges.extend(worktree_badge(session).map(str::to_owned));
        badges.push(status_label(Some(session)));
    }
    if focused {
        badges.push("typing to the agent".to_owned());
    }
    if let Some(offset) = scrolled {
        badges.push(format!("↑{offset}"));
    }
    badges.retain(|badge| !badge.is_empty());
    // Keep the header on one row: the badge yields to at least 8 columns of path.
    let badge = truncate(&badges.join(" · "), width.saturating_sub(9));
    let badge = (!badge.is_empty()).then_some(badge);
    let path_budget = width
        .saturating_sub(badge.as_ref().map_or(0, |badge| js_len(badge) + 1))
        .max(1);
    let cwd_label = match session {
        Some(session) => {
            let source = session
                .worktree
                .as_ref()
                .and_then(|worktree| worktree.path.as_deref())
                .unwrap_or(&session.cwd);
            compact_path(source, path_budget)
        }
        None => "Select a session from the sidebar".to_owned(),
    };
    let left = vec![Seg::new(truncate(&cwd_label, path_budget), TextStyle::fg(THEME.muted))];
    let right: Row = badge.map_or_else(Vec::new, |badge| {
        let row = vec![Seg::new(badge, TextStyle::fg(THEME.accent))];
        truncate_row(&row, width, Wrap::TruncateEnd)
    });
    space_between(&left, &right, width)
}

/// TS: preview.tsx PreviewPane's rows at `width` × `height`: the path/badge header, then the body.
pub fn preview_rows(props: &PreviewPaneProps<'_>, width: usize, height: usize) -> Vec<Row> {
    let PreviewPaneProps { session, preview, .. } = *props;
    let body_height = height.saturating_sub(1).max(1);
    let lines = fit_lines(&fallback_message(session, preview, body_height), width, body_height);
    let has_content =
        session.is_some_and(|session| !preview.content.is_empty() || session.status == SessionStatus::Exited);
    let mut rows = vec![header_row(props, width)];
    for (index, line) in lines.into_iter().enumerate() {
        let color = if has_content {
            None
        } else if index == 0 {
            Some(THEME.active)
        } else {
            Some(THEME.muted)
        };
        let row = vec![Seg::new(line, TextStyle::default().color(color))];
        // A wide character can push a line past the box: Ink wraps it.
        if row_width(&row) > width {
            rows.extend(fit_row(&row, width, Wrap::Wrap));
        } else {
            rows.push(row);
        }
    }
    rows.truncate(height);
    rows
}

/// TS: preview.tsx PreviewPane. In preview focus, once the styled screen arrived, the body is the agent's screen in
/// its colours ([`render_screen`]); until then, and outside focus, the plain text.
pub fn render_preview_pane(props: &PreviewPaneProps<'_>, buf: &mut Buffer, area: Rect) {
    match props.preview.screen.as_ref().filter(|_| props.focused) {
        Some(screen) if area.height > 1 => {
            let header = header_row(props, usize::from(area.width));
            draw_rows(buf, Rect { height: 1, ..area }, &[header]);
            render_screen(screen, buf, preview_body(area));
        }
        _ => {
            let rows = preview_rows(props, usize::from(area.width), usize::from(area.height));
            draw_rows(buf, area, &rows);
        }
    }
}

/// Where the Preview tab's body (the agent's screen) is drawn in a pane at `area`: below its header row.
pub fn preview_body(area: Rect) -> Rect {
    Rect {
        y: area.y.saturating_add(1),
        height: area.height.saturating_sub(1),
        ..area
    }
}

/// The agent's screen in `area`: each segment at its emulator column, fitted to its cells (the emulator and the UI
/// can measure a character's width differently, so placing by column keeps the rest of the row in place).
pub fn render_screen(screen: &PreviewScreen, buf: &mut Buffer, area: Rect) {
    for (row, line) in screen.lines.iter().take(usize::from(area.height)).enumerate() {
        let y = area.y + u16::try_from(row).unwrap_or(u16::MAX);
        for segment in line {
            if segment.x >= area.width {
                continue;
            }
            let x = area.x + segment.x;
            let cells = segment.cells.min(area.width - segment.x);
            let style = cell_style(segment.attrs);
            let (end, _) = buf.set_stringn(x, y, &segment.text, usize::from(cells), style);
            // Styled blanks and a narrower rendering keep the segment's background to its last cell.
            for pad in end..x + cells {
                buf[(pad, y)].set_symbol(" ").set_style(style);
            }
        }
    }
}

/// An emulator cell's attributes as a ratatui style. The 16 ANSI colours stay named, so the terminal's palette
/// (and its own default colours) apply, as they would attached.
pub fn cell_style(attrs: CellAttrs) -> Style {
    let flags = [
        (CellFlags::BOLD, Modifier::BOLD),
        (CellFlags::DIM, Modifier::DIM),
        (CellFlags::ITALIC, Modifier::ITALIC),
        (CellFlags::UNDERLINE, Modifier::UNDERLINED),
        (CellFlags::BLINK, Modifier::SLOW_BLINK),
        (CellFlags::INVERSE, Modifier::REVERSED),
        (CellFlags::INVISIBLE, Modifier::HIDDEN),
        (CellFlags::STRIKETHROUGH, Modifier::CROSSED_OUT),
    ];
    let modifier = flags
        .into_iter()
        .filter(|(flag, _)| attrs.flags.contains(*flag))
        .fold(Modifier::empty(), |modifier, (_, add)| modifier | add);
    Style::default()
        .fg(term_color(attrs.fg))
        .bg(term_color(attrs.bg))
        .add_modifier(modifier)
}

fn term_color(color: Color) -> TermColor {
    const ANSI: [TermColor; 16] = [
        TermColor::Black,
        TermColor::Red,
        TermColor::Green,
        TermColor::Yellow,
        TermColor::Blue,
        TermColor::Magenta,
        TermColor::Cyan,
        TermColor::Gray,
        TermColor::DarkGray,
        TermColor::LightRed,
        TermColor::LightGreen,
        TermColor::LightYellow,
        TermColor::LightBlue,
        TermColor::LightMagenta,
        TermColor::LightCyan,
        TermColor::White,
    ];
    match color {
        Color::Default => TermColor::Reset,
        Color::P16(index) => ANSI.get(usize::from(index)).copied().unwrap_or(TermColor::Reset),
        Color::P256(index) => TermColor::Indexed(index),
        Color::Rgb(r, g, b) => TermColor::Rgb(r, g, b),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::row_text;
    use serde_json::json;

    fn session(extra: serde_json::Value) -> SessionRecord {
        let mut full = json!({
            "id": "s1", "title": "t", "program": "claude", "command": "claude", "cwd": "/work/repo",
            "repoRoot": "/work/repo", "status": "running", "createdAt": "", "updatedAt": "",
        });
        if let (Some(full), Some(extra)) = (full.as_object_mut(), extra.as_object()) {
            for (key, value) in extra {
                full.insert(key.clone(), value.clone());
            }
        }
        serde_json::from_value(full).expect("session")
    }

    fn preview(content: &str) -> PreviewRecord {
        PreviewRecord {
            session_id: None,
            content: content.to_owned(),
            live: true,
            status: None,
            agent_status: None,
            scroll_offset: None,
            max_scroll_offset: None,
            screen: None,
        }
    }

    fn interactive_screen() -> PreviewScreen {
        serde_json::from_value(json!({
            "lines": [
                [[0, 12, "✻ Welcome to", "rgb:d97757", "d", "b"], [13, 6, "Claude", "d", "d", ""]],
                [],
                [[0, 2, "> ", "p16:8", "d", ""], [2, 5, "hello", "d", "d", "b"], [10, 4, "日本", "p16:2", "d", ""]],
                [[0, 18, "  esc to interrupt", "p256:244", "d", "d"]],
                [[0, 6, "      ", "d", "p16:4", ""], [7, 4, "done", "p16:0", "p16:7", "r"]],
            ],
            "cursor": {"x": 7, "y": 2},
            "modes": {"bracketedPaste": true, "applicationCursorKeys": false, "mouseTracking": false},
        }))
        .expect("screen")
    }

    #[test]
    fn typing_to_the_agent_matches_the_snapshot() {
        use crate::flows::tasks::tests::{buffer_rows, style_rows};
        let running = session(json!({}));
        let mut record = preview("✻ Welcome to Claude\n\n> hello   日本\n  esc to interrupt\n       done");
        record.screen = Some(interactive_screen());
        let props = PreviewPaneProps {
            session: Some(&running),
            preview: &record,
            spinner_frame: "",
            focused: true,
        };
        let area = Rect::new(0, 0, 98, 31);
        let mut buf = Buffer::empty(area);
        render_preview_pane(&props, &mut buf, area);
        let body = preview_body(area);
        let cursor = record.screen.as_ref().and_then(|screen| screen.cursor).expect("cursor");
        let mut text = buffer_rows(&buf);
        text.push("── styles".to_owned());
        text.extend(style_rows(&buf));
        text.push("── cursor".to_owned());
        text.push(format!("{},{}", body.x + cursor.x, body.y + cursor.y));
        deckhand_testkit::assert_snapshot("fixtures/ui/panes/60-preview-interactive.txt", &text.join("\n"));
    }

    #[test]
    fn the_styled_screen_is_drawn_only_in_focus() {
        let running = session(json!({}));
        let mut record = preview("plain");
        record.screen = Some(interactive_screen());
        let area = Rect::new(0, 0, 40, 6);
        let draw = |focused: bool| {
            let props = PreviewPaneProps {
                session: Some(&running),
                preview: &record,
                spinner_frame: "",
                focused,
            };
            let mut buf = Buffer::empty(area);
            render_preview_pane(&props, &mut buf, area);
            buf
        };
        let mut plain = Buffer::empty(area);
        let props = PreviewPaneProps {
            session: Some(&running),
            preview: &PreviewRecord {
                screen: None,
                ..record.clone()
            },
            spinner_frame: "",
            focused: false,
        };
        draw_rows(&mut plain, area, &preview_rows(&props, 40, 6));
        assert_eq!(draw(false), plain, "unfocused: the plain text, as before");
        let focused = draw(true);
        assert_eq!(focused[(0, 1)].symbol(), "✻");
        assert_eq!(focused[(0, 1)].fg, TermColor::Rgb(0xd9, 0x77, 0x57));
        // The wide characters stay at their emulator columns; the segment's last cell is padded.
        assert_eq!(focused[(10, 3)].symbol(), "日");
        assert_eq!(focused[(12, 3)].symbol(), "本");
    }

    #[test]
    fn cell_styles_keep_the_terminals_palette() {
        let style = cell_style(CellAttrs {
            fg: Color::P16(9),
            bg: Color::P256(17),
            flags: CellFlags::BOLD | CellFlags::INVERSE | CellFlags::UNDERLINE,
        });
        assert_eq!(style.fg, Some(TermColor::LightRed));
        assert_eq!(style.bg, Some(TermColor::Indexed(17)));
        assert_eq!(
            style.add_modifier,
            Modifier::BOLD | Modifier::REVERSED | Modifier::UNDERLINED
        );
        assert_eq!(cell_style(CellAttrs::default()).fg, Some(TermColor::Reset));
    }

    #[test]
    fn fallback_messages() {
        let empty = preview("");
        assert_eq!(
            fallback_message(None, &empty, 5),
            "No sessions yet. Press n to create one."
        );
        assert_eq!(
            fallback_message(Some(&session(json!({"status": "starting"}))), &empty, 5),
            "Starting session…"
        );
        assert_eq!(
            fallback_message(Some(&session(json!({}))), &empty, 5),
            "Waiting for agent output…"
        );
        assert_eq!(
            fallback_message(Some(&session(json!({"status": "exited"}))), &empty, 5),
            "Session exited."
        );
        assert_eq!(fallback_message(Some(&session(json!({}))), &preview("hi"), 5), "hi");
    }

    #[test]
    fn setup_output_shows_its_tail_below_the_header() {
        let starting = session(json!({
            "status": "starting",
            "setup": {"command": "npm i", "state": "running", "output": "a\nb\nc\n\n"},
        }));
        assert_eq!(
            fallback_message(Some(&starting), &preview(""), 3),
            "Setup (running): npm i\nb\nc"
        );
        assert_eq!(
            fallback_message(Some(&starting), &preview(""), 1),
            "Setup (running): npm i"
        );
        let failed = session(json!({
            "status": "exited",
            "setup": {"command": "npm i", "state": "failed", "output": "boom"},
        }));
        assert_eq!(
            fallback_message(Some(&failed), &preview("one\ntwo\nthree"), 4),
            "one\ntwo\nSetup (failed): npm i\nboom"
        );
        assert_eq!(
            fallback_message(Some(&failed), &preview("Setup (failed): x"), 4),
            "Setup (failed): npm i\nboom"
        );
    }

    #[test]
    fn badges_and_header() {
        let managed = session(json!({"worktree": {"mode": "managed", "path": "/wt/a"}}));
        assert_eq!(worktree_badge(&managed), Some("worktree"));
        assert_eq!(
            worktree_badge(&session(json!({"worktree": {"mode": "attached"}}))),
            Some("attached")
        );
        assert_eq!(worktree_badge(&session(json!({"worktree": {"mode": "none"}}))), None);
        let mut record = preview("hello");
        record.scroll_offset = Some(3);
        let props = PreviewPaneProps {
            session: Some(&managed),
            preview: &record,
            spinner_frame: "",
            focused: true,
        };
        let rows = preview_rows(&props, 60, 3);
        assert_eq!(rows.len(), 3);
        let header = row_text(&rows[0]);
        assert!(header.starts_with("/wt/a"));
        assert!(
            header.ends_with("worktree · running · typing to the agent · ↑3"),
            "{header}"
        );
        assert_eq!(row_text(&rows[1]), "hello");
        assert_eq!(rows[1][0].style.color, None);
    }

    #[test]
    fn without_a_session_the_hint_is_highlighted() {
        let record = preview("");
        let props = PreviewPaneProps {
            session: None,
            preview: &record,
            spinner_frame: "",
            focused: false,
        };
        let rows = preview_rows(&props, 40, 3);
        assert_eq!(row_text(&rows[0]).trim_end(), "Select a session from the sidebar");
        assert_eq!(row_text(&rows[1]), "No sessions yet. Press n to create one.");
        assert_eq!(rows[1][0].style.color, Some(THEME.active));
        assert_eq!(rows[2][0].style.color, Some(THEME.muted));
    }
}
