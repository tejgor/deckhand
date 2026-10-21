//! The Terminal tab (the workspace's shell, or the last action run there; v switches).

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;

use deckhand_core::display::{compact_path, fit_lines, truncate};
use deckhand_core::jsstr::js_len;
use deckhand_core::workspace::workspace_pane_unavailable;
use deckhand_protocol::data::{ActionRecord, SessionRecord, TerminalRecord};

use crate::primitives::{Row, Seg, TextStyle, draw_rows, space_between};
use crate::theme::THEME;

/// TS: terminalPane.tsx TerminalView: what the Terminal tab shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum TerminalView {
    #[default]
    Shell,
    Action,
}

/// TS: terminalPane.tsx hasAction: the selected session's workspace has an action to show (running, or finished with
/// its output kept).
pub fn has_action(session: Option<&SessionRecord>, action: &ActionRecord) -> bool {
    let filled = |value: &Option<String>| value.as_deref().is_some_and(|text| !text.is_empty());
    session.is_some_and(|session| {
        action.session_id.as_deref() == Some(session.id.as_str())
            && filled(&action.workspace)
            && filled(&action.command)
    })
}

/// TS: terminalPane.tsx actionStatus's result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActionStatus {
    pub text: String,
    pub color: Color,
}

/// TS: terminalPane.tsx actionStatus: running, or how it ended (`● running`, `✓ exit 0`, `✗ signal 9`, ...).
pub fn action_status(action: &ActionRecord) -> ActionStatus {
    let status = |text: String, color: Color| ActionStatus { text, color };
    if action.live {
        return status("● running".to_owned(), THEME.success);
    }
    // `null` and absent are both "no value", as for a TS number check.
    let exit_code = action.exit_code.flatten();
    let exit_signal = action.exit_signal.flatten().filter(|signal| *signal != 0);
    if exit_code == Some(0) && exit_signal.is_none() {
        return status("✓ exit 0".to_owned(), THEME.success);
    }
    if let Some(signal) = exit_signal {
        return status(format!("✗ signal {signal}"), THEME.error);
    }
    if let Some(code) = exit_code {
        return status(format!("✗ exit {code}"), THEME.error);
    }
    status("○ stopped".to_owned(), THEME.muted)
}

/// TS: terminalPane.tsx fallbackMessage: the shell's output, or why there is none.
pub fn fallback_message(session: Option<&SessionRecord>, terminal: &TerminalRecord) -> String {
    // The shell belongs to the session's workspace (worktree) and is shared by every session in it, running or not.
    let Some(session) = session else {
        return "No session selected.".to_owned();
    };
    if let Some(unavailable) =
        workspace_pane_unavailable(session, terminal.session_id.as_deref(), terminal.workspace.as_deref())
    {
        return format!("Terminal is unavailable: {unavailable}.");
    }
    if !terminal.live && !terminal.content.is_empty() {
        return terminal.content.clone();
    }
    if !terminal.live && terminal.session_id.as_deref() == Some(session.id.as_str()) {
        return "Shell exited. Switch tabs and back to start a new one.".to_owned();
    }
    if terminal.content.is_empty() {
        "Starting terminal…".to_owned()
    } else {
        terminal.content.clone()
    }
}

/// TS: terminalPane.tsx TerminalPaneProps (width and height are the area's).
#[derive(Clone, Copy, Debug)]
pub struct TerminalPaneProps<'a> {
    pub session: Option<&'a SessionRecord>,
    pub terminal: &'a TerminalRecord,
    pub action: &'a ActionRecord,
    pub view: TerminalView,
}

/// TS: terminalPane.tsx TerminalPane's rows: the path / view header, then the body.
pub fn terminal_rows(props: &TerminalPaneProps<'_>, width: usize, height: usize) -> Vec<Row> {
    let TerminalPaneProps {
        session,
        terminal,
        action,
        view,
    } = *props;
    let body_height = height.saturating_sub(1).max(1);
    let with_action = has_action(session, action);
    let show_action = with_action && view == TerminalView::Action;
    let body = if show_action {
        if !action.content.is_empty() {
            action.content.clone()
        } else if action.live {
            "Running…".to_owned()
        } else {
            "(no output)".to_owned()
        }
    } else {
        fallback_message(session, terminal)
    };
    let lines = fit_lines(&body, width, body_height);
    // With an action, the header names both views (the shown one highlighted) and the action's state.
    let shell_status = if terminal.live { "● live" } else { "○ cold" };
    let status = action_status(action);
    let action_name = action.name.as_deref().unwrap_or("action");
    let right_text = if with_action {
        format!("shell │ {action_name} {}", status.text)
    } else {
        shell_status.to_owned()
    };
    let path_source = if show_action {
        format!("$ {}", action.command.as_deref().unwrap_or_default())
    } else {
        terminal
            .cwd
            .clone()
            .or_else(|| {
                session.and_then(|session| session.worktree.as_ref().and_then(|worktree| worktree.path.clone()))
            })
            .or_else(|| session.map(|session| session.cwd.clone()))
            .unwrap_or_else(|| "Select a session from the sidebar.".to_owned())
    };
    let left_budget = width.saturating_sub(js_len(&right_text) + 1).max(8);
    let left = if show_action {
        truncate(&path_source, left_budget)
    } else {
        compact_path(&path_source, left_budget)
    };
    let left = vec![Seg::new(truncate(&left, left_budget), TextStyle::fg(THEME.muted))];
    let right: Row = if with_action {
        let shell = TextStyle::fg(if show_action { THEME.muted } else { THEME.active }).bold(!show_action);
        let name = TextStyle::fg(if show_action { THEME.active } else { THEME.muted }).bold(show_action);
        vec![
            Seg::new("shell", shell),
            Seg::new(" │ ", TextStyle::fg(THEME.muted)),
            Seg::new(format!("{action_name} "), name),
            Seg::new(status.text, TextStyle::fg(status.color)),
        ]
    } else {
        let color = if terminal.live { THEME.success } else { THEME.muted };
        vec![Seg::new(shell_status, TextStyle::fg(color))]
    };
    let mut rows = vec![space_between(&left, &right, width)];
    rows.extend(lines.into_iter().map(|line| vec![Seg::plain(line)]));
    rows
}

/// TS: terminalPane.tsx TerminalPane.
pub fn render_terminal_pane(props: &TerminalPaneProps<'_>, buf: &mut Buffer, area: Rect) {
    let rows = terminal_rows(props, usize::from(area.width), usize::from(area.height));
    draw_rows(buf, area, &rows);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::{row_text, string_width};
    use deckhand_testkit::{Case, replay, tags};
    use serde_json::{Value, json};

    fn session(extra: Value) -> SessionRecord {
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

    /// A pane record from a partial TS literal (`content` and `live` default as a test literal omits them).
    fn pane(value: &Value) -> Result<ActionRecord, String> {
        let mut full = json!({"content": "", "live": false});
        if let (Some(full), Some(given)) = (full.as_object_mut(), value.as_object()) {
            for (key, entry) in given {
                full.insert(key.clone(), entry.clone());
            }
        }
        serde_json::from_value(full).map_err(|error| error.to_string())
    }

    fn ink_name(color: Color) -> &'static str {
        match color {
            Color::Green => "green",
            Color::Red => "red",
            Color::DarkGray => "gray",
            _ => "other",
        }
    }

    #[test]
    fn has_action_parity() {
        replay("terminalPane/hasAction", |case: &Case| -> Result<bool, String> {
            let session_arg = tags::untag(case.arg(0));
            let session = session_arg
                .get("id")
                .and_then(Value::as_str)
                .map(|id| session(json!({"id": id})));
            Ok(has_action(session.as_ref(), &pane(&tags::untag(case.arg(1)))?))
        });
    }

    #[test]
    fn action_status_parity() {
        replay("terminalPane/actionStatus", |case: &Case| -> Result<Value, String> {
            let status = action_status(&pane(&tags::untag(case.arg(0)))?);
            Ok(json!({"text": status.text, "color": ink_name(status.color)}))
        });
    }

    #[test]
    fn fallback_messages() {
        let record = |content: &str, live: bool, id: Option<&str>| TerminalRecord {
            content: content.to_owned(),
            live,
            session_id: id.map(str::to_owned),
            workspace: Some("/work/wt".to_owned()),
            ..TerminalRecord::default()
        };
        let session = session(json!({}));
        assert_eq!(fallback_message(None, &record("", false, None)), "No session selected.");
        assert_eq!(
            fallback_message(Some(&session), &record("", false, None)),
            "Starting terminal…"
        );
        assert_eq!(
            fallback_message(Some(&session), &record("", false, Some("s1"))),
            "Shell exited. Switch tabs and back to start a new one."
        );
        assert_eq!(
            fallback_message(Some(&session), &record("out", false, Some("s1"))),
            "out"
        );
        let no_workspace = TerminalRecord {
            workspace: None,
            ..record("", true, Some("s1"))
        };
        assert!(fallback_message(Some(&session), &no_workspace).starts_with("Terminal is unavailable: "));
    }

    #[test]
    fn header_names_both_views_when_an_action_exists() {
        let session = session(json!({}));
        let terminal = TerminalRecord {
            content: "$ ".to_owned(),
            live: true,
            cwd: Some("/work/wt".to_owned()),
            session_id: Some("s1".to_owned()),
            workspace: Some("/work/wt".to_owned()),
            ..TerminalRecord::default()
        };
        let action = ActionRecord {
            content: String::new(),
            live: false,
            command: Some("npm test".to_owned()),
            name: Some("test".to_owned()),
            exit_code: Some(Some(1)),
            session_id: Some("s1".to_owned()),
            workspace: Some("/work/wt".to_owned()),
            ..ActionRecord::default()
        };
        let mut props = TerminalPaneProps {
            session: Some(&session),
            terminal: &terminal,
            action: &action,
            view: TerminalView::Shell,
        };
        let rows = terminal_rows(&props, 50, 3);
        let header = row_text(&rows[0]);
        assert!(header.starts_with("/work/wt  "));
        assert!(header.ends_with(" shell │ test ✗ exit 1"));
        assert_eq!(string_width(&header), 50);
        assert_eq!(row_text(&rows[1]), "$ ");
        let shell = rows[0].iter().find(|seg| seg.text == "shell").map(|seg| seg.style.bold);
        assert_eq!(shell, Some(Some(true)));
        props.view = TerminalView::Action;
        let rows = terminal_rows(&props, 50, 3);
        assert!(row_text(&rows[0]).starts_with("$ npm test  "));
        assert_eq!(row_text(&rows[1]), "(no output)");
        let plain = TerminalPaneProps {
            action: &ActionRecord::default(),
            ..props
        };
        let rows = terminal_rows(&plain, 30, 2);
        assert_eq!(row_text(&rows[0]), "/work/wt                ● live");
    }
}
