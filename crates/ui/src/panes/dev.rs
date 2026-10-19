//! Port of devPane.tsx: the Dev tab (the workspace's Dev command, shared by every session in the worktree).

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use deckhand_core::display::{compact_path, fit_lines, truncate};
use deckhand_core::jsstr::js_len;
use deckhand_core::workspace::workspace_pane_unavailable;
use deckhand_protocol::data::{DevRecord, SessionRecord};

use crate::primitives::{Row, Seg, TextStyle, draw_rows, space_between};
use crate::theme::THEME;

/// TS: devPane.tsx fallbackMessage: the Dev output, or why there is none.
pub fn fallback_message(session: Option<&SessionRecord>, dev: &DevRecord) -> String {
    let Some(session) = session else {
        return "No session selected.".to_owned();
    };
    if let Some(unavailable) = workspace_pane_unavailable(session, dev.session_id.as_deref(), dev.workspace.as_deref())
    {
        return format!("Dev is unavailable: {unavailable}.");
    }
    if !dev.live && !dev.content.is_empty() {
        return dev.content.clone();
    }
    if !dev.live && dev.session_id.as_deref() == Some(session.id.as_str()) {
        return "Dev command exited. d d starts it again (press d twice quickly).".to_owned();
    }
    if dev.content.is_empty() {
        "d d starts the Dev command (press d twice quickly; shared by every session in this worktree).".to_owned()
    } else {
        dev.content.clone()
    }
}

/// TS: devPane.tsx DevPaneProps (width and height are the area's).
#[derive(Clone, Copy, Debug)]
pub struct DevPaneProps<'a> {
    pub session: Option<&'a SessionRecord>,
    pub dev: &'a DevRecord,
}

/// TS: devPane.tsx DevPane's rows: the path · command / status header, then the body.
pub fn dev_rows(props: &DevPaneProps<'_>, width: usize, height: usize) -> Vec<Row> {
    let DevPaneProps { session, dev } = *props;
    let body_height = height.saturating_sub(1).max(1);
    let lines = fit_lines(&fallback_message(session, dev), width, body_height);
    let status = if dev.live { "● live" } else { "○ idle" };
    let command = match dev.command.as_deref() {
        Some(command) if !command.is_empty() => format!(" · {command}"),
        _ => String::new(),
    };
    let path_source = dev
        .cwd
        .as_deref()
        .or_else(|| session.and_then(|session| session.worktree.as_ref().and_then(|worktree| worktree.path.as_deref())))
        .or_else(|| session.map(|session| session.cwd.as_str()))
        .unwrap_or("Select a session from the sidebar.");
    let command_len = js_len(&command);
    let cwd_budget = width.saturating_sub(js_len(status) + 1 + command_len).max(8);
    let cwd = compact_path(path_source, cwd_budget);
    let left = vec![Seg::new(
        truncate(&format!("{cwd}{command}"), cwd_budget + command_len),
        TextStyle::fg(THEME.muted),
    )];
    let live_color = if dev.live { THEME.success } else { THEME.muted };
    let right = vec![Seg::new(status, TextStyle::fg(live_color))];
    let mut rows = vec![space_between(&left, &right, width)];
    rows.extend(lines.into_iter().map(|line| vec![Seg::plain(line)]));
    rows
}

/// TS: devPane.tsx DevPane.
pub fn render_dev_pane(props: &DevPaneProps<'_>, buf: &mut Buffer, area: Rect) {
    let rows = dev_rows(props, usize::from(area.width), usize::from(area.height));
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
            "worktree": {"mode": "managed", "path": "/work/wt"},
        });
        if let (Some(full), Some(extra)) = (full.as_object_mut(), extra.as_object()) {
            for (key, value) in extra {
                full.insert(key.clone(), value.clone());
            }
        }
        serde_json::from_value(full).expect("session")
    }

    fn dev(content: &str, live: bool, session_id: Option<&str>) -> DevRecord {
        DevRecord {
            content: content.to_owned(),
            live,
            session_id: session_id.map(str::to_owned),
            workspace: Some("/work/wt".to_owned()),
            ..DevRecord::default()
        }
    }

    #[test]
    fn fallback_messages() {
        let session = session(json!({}));
        assert_eq!(fallback_message(None, &dev("", false, None)), "No session selected.");
        assert_eq!(
            fallback_message(Some(&session), &dev("", false, None)),
            "d d starts the Dev command (press d twice quickly; shared by every session in this worktree)."
        );
        assert_eq!(
            fallback_message(Some(&session), &dev("", false, Some("s1"))),
            "Dev command exited. d d starts it again (press d twice quickly)."
        );
        assert_eq!(fallback_message(Some(&session), &dev("out", false, Some("s1"))), "out");
        assert_eq!(
            fallback_message(Some(&session), &dev("live out", true, Some("s1"))),
            "live out"
        );
        let no_workspace = DevRecord {
            workspace: None,
            ..dev("", false, Some("s1"))
        };
        assert!(fallback_message(Some(&session), &no_workspace).starts_with("Dev is unavailable: "));
    }

    #[test]
    fn header_shows_path_command_and_status() {
        let session = session(json!({}));
        let record = DevRecord {
            command: Some("npm run dev".to_owned()),
            ..dev("ready", true, Some("s1"))
        };
        let rows = dev_rows(
            &DevPaneProps {
                session: Some(&session),
                dev: &record,
            },
            40,
            3,
        );
        assert_eq!(rows.len(), 3);
        assert_eq!(row_text(&rows[0]), "/work/wt · npm run dev            ● live");
        assert_eq!(row_text(&rows[1]), "ready");
        assert_eq!(row_text(&rows[2]), " ");
        let idle = dev_rows(
            &DevPaneProps {
                session: None,
                dev: &DevRecord::default(),
            },
            50,
            2,
        );
        assert!(row_text(&idle[0]).starts_with("Select a session from the sidebar."));
        assert!(row_text(&idle[0]).ends_with("○ idle"));
    }
}
