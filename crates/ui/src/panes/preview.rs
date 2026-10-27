//! The Preview tab (the agent's screen, or what a session that is not running leaves: setup output, its last preview, a
//! hint).

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use deckhand_core::display::{compact_path, fit_lines, plain_terminal_text, status_label, truncate};
use deckhand_core::jsstr::{js_len, js_trim};
use deckhand_protocol::data::{
    PreviewRecord, SessionRecord, SessionSetup, SessionStatus, SessionWorktreeMode, SetupState,
};

use crate::primitives::{Row, Seg, TextStyle, Wrap, draw_rows, fit_row, row_width, space_between, truncate_row};
use crate::theme::THEME;

/// TS: preview.tsx PreviewPaneProps (width and height are the area's).
#[derive(Clone, Copy, Debug)]
pub struct PreviewPaneProps<'a> {
    pub session: Option<&'a SessionRecord>,
    pub preview: &'a PreviewRecord,
    pub spinner_frame: &'a str,
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

/// TS: preview.tsx PreviewPane's rows at `width` × `height`: the path/badge header, then the body.
pub fn preview_rows(props: &PreviewPaneProps<'_>, width: usize, height: usize) -> Vec<Row> {
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
        badges.push(match scrolled {
            Some(offset) => format!("preview ↑{offset}"),
            None => "preview focus".to_owned(),
        });
    } else if let Some(offset) = scrolled {
        badges.push(format!("↑{offset}"));
    }
    badges.retain(|badge| !badge.is_empty());
    // Keep the header on one row: the badge yields to at least 8 columns of path.
    let badge = truncate(&badges.join(" · "), width.saturating_sub(9));
    let badge = (!badge.is_empty()).then_some(badge);
    let body_height = height.saturating_sub(1).max(1);
    let lines = fit_lines(&fallback_message(session, preview, body_height), width, body_height);
    let has_content =
        session.is_some_and(|session| !preview.content.is_empty() || session.status == SessionStatus::Exited);
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
    let mut rows = vec![space_between(&left, &right, width)];
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

/// TS: preview.tsx PreviewPane.
pub fn render_preview_pane(props: &PreviewPaneProps<'_>, buf: &mut Buffer, area: Rect) {
    let rows = preview_rows(props, usize::from(area.width), usize::from(area.height));
    draw_rows(buf, area, &rows);
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
        let rows = preview_rows(&props, 50, 3);
        assert_eq!(rows.len(), 3);
        let header = row_text(&rows[0]);
        assert!(header.starts_with("/wt/a"));
        assert!(header.ends_with("worktree · running · preview ↑3"));
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
