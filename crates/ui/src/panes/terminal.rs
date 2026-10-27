//! The Terminal tab (one of the workspace's shells, or the last action run there; v switches). While you type into
//! a shell (→) it draws that shell's screen in its own colours.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;

use deckhand_core::display::{compact_path, fit_lines, truncate};
use deckhand_core::jsstr::js_len;
use deckhand_core::workspace::workspace_pane_unavailable;
use deckhand_protocol::data::{ActionRecord, SessionRecord, TerminalRecord};
use deckhand_protocol::screen::PreviewScreen;

use crate::panes::preview::{preview_body, render_screen};
use crate::primitives::{Row, Seg, TextStyle, draw_rows, space_between};
use crate::theme::THEME;

/// TS: terminalPane.tsx TerminalView: what the Terminal tab shows. Not in the TS: which shell, by id (0 is the
/// first).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TerminalView {
    Shell(i64),
    Action,
}

impl Default for TerminalView {
    fn default() -> Self {
        TerminalView::Shell(0)
    }
}

/// Not in the TS: how many shells a worktree runs at most (the workspace worker refuses more).
pub const MAX_SHELLS: usize = 4;

/// Not in the TS: one of the worktree's shells as the Terminal tab shows it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShellView<'a> {
    pub id: i64,
    pub content: &'a str,
    pub live: bool,
    pub screen: Option<&'a PreviewScreen>,
}

/// Not in the TS: the record's shells, the first first (just the record's own with a single shell).
pub fn shells(terminal: &TerminalRecord) -> Vec<ShellView<'_>> {
    match &terminal.shells {
        Some(shells) if !shells.is_empty() => shells
            .iter()
            .map(|shell| ShellView {
                id: shell.id,
                content: &shell.content,
                live: shell.live,
                screen: shell.screen.as_ref(),
            })
            .collect(),
        _ => vec![ShellView {
            id: 0,
            content: &terminal.content,
            live: terminal.live,
            screen: terminal.screen.as_ref(),
        }],
    }
}

/// Not in the TS: the shell `view` shows: its own while it runs, else the first.
pub fn shown_shell(terminal: &TerminalRecord, view: TerminalView) -> i64 {
    match view {
        TerminalView::Shell(id) if shells(terminal).iter().any(|shell| shell.id == id) => id,
        _ => 0,
    }
}

/// Not in the TS: a shell's label, by position: `shell` alone, else `shell 1`, `shell 2`, ...
pub fn shell_label(terminal: &TerminalRecord, id: i64) -> String {
    let all = shells(terminal);
    if all.len() < 2 {
        return "shell".to_owned();
    }
    let position = all.iter().position(|shell| shell.id == id).unwrap_or(0);
    format!("shell {}", position + 1)
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
    /// Not in the TS: you type into the shown shell; its styled screen is drawn once it arrives.
    pub focused: bool,
}

/// TS: terminalPane.tsx TerminalPane's rows: the path / view header, then the body.
pub fn terminal_rows(props: &TerminalPaneProps<'_>, width: usize, height: usize) -> Vec<Row> {
    let TerminalPaneProps {
        session,
        terminal,
        action,
        view,
        focused,
    } = *props;
    let body_height = height.saturating_sub(1).max(1);
    let with_action = has_action(session, action);
    let show_action = with_action && view == TerminalView::Action;
    let all = shells(terminal);
    let shown = shown_shell(terminal, view);
    let shown_view = all.iter().find(|shell| shell.id == shown).copied();
    let body = if show_action {
        if !action.content.is_empty() {
            action.content.clone()
        } else if action.live {
            "Running…".to_owned()
        } else {
            "(no output)".to_owned()
        }
    } else if let Some(shell) = shown_view.filter(|shell| shell.id != 0) {
        if shell.content.is_empty() {
            "Starting shell…".to_owned()
        } else {
            shell.content.to_owned()
        }
    } else {
        fallback_message(session, terminal)
    };
    let lines = fit_lines(&body, width, body_height);
    // With an action or several shells, the header names every view (the shown one highlighted) and the action's
    // state; typing into a shell says so instead.
    let live = shown_view.is_some_and(|shell| shell.live);
    let shell_status = if live { "● live" } else { "○ cold" };
    let status = action_status(action);
    let action_name = action.name.as_deref().unwrap_or("action");
    let labels: Vec<String> = if all.len() > 1 {
        all.iter().map(|shell| shell_label(terminal, shell.id)).collect()
    } else {
        vec!["shell".to_owned()]
    };
    let typing = focused && !show_action;
    let right_text = if typing {
        format!("typing into {}", shell_label(terminal, shown))
    } else if with_action {
        format!("{} │ {action_name} {}", labels.join(" │ "), status.text)
    } else if all.len() > 1 {
        format!("{} {shell_status}", labels.join(" │ "))
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
    let label_segs = |segs: &mut Row| {
        for (index, (shell, label)) in all.iter().zip(&labels).enumerate() {
            if index > 0 {
                segs.push(Seg::new(" │ ", TextStyle::fg(THEME.muted)));
            }
            let selected = !show_action && shell.id == shown;
            segs.push(Seg::new(
                label.clone(),
                TextStyle::fg(if selected { THEME.active } else { THEME.muted }).bold(selected),
            ));
        }
    };
    let right: Row = if typing {
        vec![Seg::new(right_text.clone(), TextStyle::fg(THEME.accent))]
    } else if with_action {
        let name = TextStyle::fg(if show_action { THEME.active } else { THEME.muted }).bold(show_action);
        let mut segs = Vec::new();
        label_segs(&mut segs);
        segs.push(Seg::new(" │ ", TextStyle::fg(THEME.muted)));
        segs.push(Seg::new(format!("{action_name} "), name));
        segs.push(Seg::new(status.text, TextStyle::fg(status.color)));
        segs
    } else {
        let color = if live { THEME.success } else { THEME.muted };
        let mut segs = Vec::new();
        if all.len() > 1 {
            label_segs(&mut segs);
            segs.push(Seg::plain(" "));
        }
        segs.push(Seg::new(shell_status, TextStyle::fg(color)));
        segs
    };
    let mut rows = vec![space_between(&left, &right, width)];
    rows.extend(lines.into_iter().map(|line| vec![Seg::plain(line)]));
    rows
}

/// Not in the TS: the styled screen to draw: the shown shell's, while you type into it.
pub fn typed_screen<'a>(props: &TerminalPaneProps<'a>) -> Option<&'a PreviewScreen> {
    if !props.focused || props.view == TerminalView::Action {
        return None;
    }
    let shown = shown_shell(props.terminal, props.view);
    shells(props.terminal)
        .into_iter()
        .find(|shell| shell.id == shown)
        .and_then(|shell| shell.screen)
}

/// TS: terminalPane.tsx TerminalPane. Not in the TS: while you type into a shell, once its styled screen arrived,
/// the body is that screen in its colours.
pub fn render_terminal_pane(props: &TerminalPaneProps<'_>, buf: &mut Buffer, area: Rect) {
    let rows = terminal_rows(props, usize::from(area.width), usize::from(area.height));
    if let Some(screen) = typed_screen(props).filter(|_| area.height > 1) {
        draw_rows(buf, Rect { height: 1, ..area }, &rows[..1]);
        render_screen(screen, buf, preview_body(area));
    } else {
        draw_rows(buf, area, &rows);
    }
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
            view: TerminalView::Shell(0),
            focused: false,
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

    fn two_shells() -> TerminalRecord {
        serde_json::from_value(json!({
            "content": "$ first", "live": true, "cwd": "/work/wt", "sessionId": "s1", "workspace": "/work/wt",
            "shells": [
                {"id": 0, "content": "$ first", "live": true},
                {"id": 4, "content": "$ second", "live": true,
                 "screen": {"lines": [[[0, 8, "$ second", "p16:2", "d", ""]]], "cursor": {"x": 8, "y": 0},
                            "modes": {"bracketedPaste": false, "applicationCursorKeys": false, "mouseTracking": false}}},
            ],
        }))
        .expect("record")
    }

    #[test]
    fn several_shells_are_named_by_position_and_one_is_shown() {
        let session = session(json!({}));
        let terminal = two_shells();
        assert_eq!(shell_label(&terminal, 4), "shell 2");
        assert_eq!(shell_label(&TerminalRecord::default(), 0), "shell");
        // A shell that is gone shows the first.
        assert_eq!(shown_shell(&terminal, TerminalView::Shell(9)), 0);
        let props = TerminalPaneProps {
            session: Some(&session),
            terminal: &terminal,
            action: &ActionRecord::default(),
            view: TerminalView::Shell(4),
            focused: false,
        };
        let rows = terminal_rows(&props, 50, 3);
        let header = row_text(&rows[0]);
        assert!(header.ends_with("shell 1 │ shell 2 ● live"), "{header}");
        assert_eq!(row_text(&rows[1]), "$ second");
        let second = rows[0]
            .iter()
            .find(|seg| seg.text == "shell 2")
            .map(|seg| seg.style.bold);
        assert_eq!(second, Some(Some(true)));
        assert!(typed_screen(&props).is_none(), "only while typing");

        // Typing into it: the header says so and its styled screen is drawn.
        let typing = TerminalPaneProps { focused: true, ..props };
        assert!(row_text(&terminal_rows(&typing, 50, 3)[0]).ends_with("typing into shell 2"));
        let area = Rect::new(0, 0, 50, 3);
        let mut buf = Buffer::empty(area);
        render_terminal_pane(&typing, &mut buf, area);
        assert_eq!(buf[(0, 1)].symbol(), "$");
        assert_eq!(buf[(0, 1)].fg, Color::Green);
    }
}
