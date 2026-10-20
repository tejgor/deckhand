//! Port of devPane.tsx: the Dev tab (the workspace's Dev command, shared by every session in the worktree).
//!
//! Not in the TS: named Dev processes (`devCommand` as a map) split the tab into one cell per process, side by side while
//! each column stays at least [`MIN_DEV_COLUMN`] wide, stacked otherwise. Every cell is the same size, and so is
//! every process's PTY ([`dev_cell_size`]).

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use deckhand_core::display::{compact_path, fit_lines, truncate};
use deckhand_core::jsstr::js_len;
use deckhand_core::workspace::workspace_pane_unavailable;
use deckhand_protocol::data::{DevProcessRecord, DevRecord, SessionRecord};

use crate::primitives::{Row, Seg, TextStyle, draw_rows, space_between};
use crate::theme::THEME;

/// The narrowest column a Dev process gets side by side; narrower, the processes are stacked.
pub const MIN_DEV_COLUMN: u16 = 40;

/// How the Dev tab arranges named processes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DevSplit {
    /// Side by side, a `│` between them.
    Columns,
    /// One above the other.
    Rows,
}

/// The split for `count` processes in a pane `width` columns wide.
pub fn dev_split(width: u16, count: usize) -> DevSplit {
    let count = u16::try_from(count.max(1)).unwrap_or(u16::MAX);
    let dividers = count - 1;
    if width.saturating_sub(dividers) / count >= MIN_DEV_COLUMN {
        DevSplit::Columns
    } else {
        DevSplit::Rows
    }
}

/// The cells of `count` processes in `area` (the earlier cells take the remainder).
pub fn dev_cells(area: Rect, count: usize) -> Vec<Rect> {
    let count16 = u16::try_from(count.max(1)).unwrap_or(u16::MAX);
    let mut cells = Vec::with_capacity(count);
    match dev_split(area.width, count) {
        DevSplit::Columns => {
            let usable = area.width.saturating_sub(count16 - 1);
            let (base, extra) = (usable / count16, usable % count16);
            let mut x = area.x;
            for index in 0..count16 {
                let width = base + u16::from(index < extra);
                cells.push(Rect::new(x, area.y, width, area.height));
                x = x.saturating_add(width + 1);
            }
        }
        DevSplit::Rows => {
            let (base, extra) = (area.height / count16, area.height % count16);
            let mut y = area.y;
            for index in 0..count16 {
                let height = base + u16::from(index < extra);
                cells.push(Rect::new(area.x, y, area.width, height));
                y = y.saturating_add(height);
            }
        }
    }
    cells
}

/// The PTY size of each of `count` processes in a Dev pane `width` × `height` (its header row included): the
/// smallest cell, minus the cell's own header row.
pub fn dev_cell_size(width: u16, height: u16, count: usize) -> (u16, u16) {
    let cells = dev_cells(Rect::new(0, 0, width, height), count);
    let cols = cells.iter().map(|cell| cell.width).min().unwrap_or(width);
    let rows = cells.iter().map(|cell| cell.height).min().unwrap_or(height);
    (cols.max(1), rows.saturating_sub(1).max(1))
}

/// The named processes the Dev tab splits into (`None` for a single Dev command or none at all).
pub fn dev_processes(dev: &DevRecord) -> Option<&[DevProcessRecord]> {
    dev.processes.as_deref().filter(|processes| !processes.is_empty())
}

/// The index of the selected process: `selected` by name, else the first.
pub fn selected_process_index(dev: &DevRecord, selected: Option<&str>) -> usize {
    dev_processes(dev)
        .and_then(|processes| {
            processes
                .iter()
                .position(|process| Some(process.name.as_str()) == selected)
        })
        .unwrap_or(0)
}

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
    /// Not in the TS: the selected named process (v switches; enter attaches it).
    pub selected_process: Option<&'a str>,
}

/// TS: devPane.tsx DevPane's rows: the path · command / status header, then the body.
pub fn dev_rows(props: &DevPaneProps<'_>, width: usize, height: usize) -> Vec<Row> {
    let DevPaneProps { session, dev, .. } = *props;
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

/// One process's cell: `▸ name · command` and its status over its output. With `selected`, the name is the
/// accent colour (a split of one has nothing to select).
pub fn process_rows(process: &DevProcessRecord, selected: bool, width: usize, height: usize) -> Vec<Row> {
    let body_height = height.saturating_sub(1).max(1);
    let body = if process.content.is_empty() && !process.live {
        "Not running. r restarts it; d d starts every process.".to_owned()
    } else {
        process.content.clone()
    };
    let lines = fit_lines(&body, width, body_height);
    let (status, status_color) = if process.live {
        ("● live".to_owned(), THEME.success)
    } else {
        match (process.exit_code, process.exit_signal) {
            (Some(0), _) => ("✓ exit 0".to_owned(), THEME.muted),
            (Some(code), _) => (format!("✗ exit {code}"), THEME.error),
            (None, Some(signal)) if signal != 0 => (format!("✗ signal {signal}"), THEME.error),
            _ => ("○ stopped".to_owned(), THEME.muted),
        }
    };
    let marker = if selected { "▸ " } else { "  " };
    let name_style = if selected {
        TextStyle::fg(THEME.accent).bold(true)
    } else {
        TextStyle::default().bold(true)
    };
    let command = match process.command.as_deref() {
        Some(command) if !command.is_empty() => format!(" · {command}"),
        _ => String::new(),
    };
    let budget = width.saturating_sub(js_len(&status) + 1);
    let name = truncate(&format!("{marker}{}", process.name), budget);
    let command = truncate(&command, budget.saturating_sub(js_len(&name)));
    let left = vec![
        Seg::new(name, name_style),
        Seg::new(command, TextStyle::fg(THEME.muted)),
    ];
    let right = vec![Seg::new(status, TextStyle::fg(status_color))];
    let mut rows = vec![space_between(&left, &right, width)];
    rows.extend(lines.into_iter().map(|line| vec![Seg::plain(line)]));
    rows
}

/// TS: devPane.tsx DevPane.
pub fn render_dev_pane(props: &DevPaneProps<'_>, buf: &mut Buffer, area: Rect) {
    let unavailable = props.session.is_some_and(|session| {
        workspace_pane_unavailable(session, props.dev.session_id.as_deref(), props.dev.workspace.as_deref()).is_some()
    });
    let processes = dev_processes(props.dev).filter(|_| props.session.is_some() && !unavailable);
    let Some(processes) = processes else {
        let rows = dev_rows(props, usize::from(area.width), usize::from(area.height));
        draw_rows(buf, area, &rows);
        return;
    };
    let selected = selected_process_index(props.dev, props.selected_process);
    let cells = dev_cells(area, processes.len());
    for (index, (process, cell)) in processes.iter().zip(&cells).enumerate() {
        let rows = process_rows(
            process,
            index == selected && processes.len() > 1,
            usize::from(cell.width),
            usize::from(cell.height),
        );
        draw_rows(buf, *cell, &rows);
    }
    if dev_split(area.width, processes.len()) == DevSplit::Columns {
        let style = TextStyle::fg(THEME.border).to_style();
        for cell in cells.iter().skip(1) {
            let Some(x) = cell.x.checked_sub(1) else {
                continue;
            };
            for y in area.y..area.y.saturating_add(area.height) {
                if let Some(target) = buf.cell_mut((x, y)) {
                    target.set_symbol("│").set_style(style);
                }
            }
        }
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

    fn process(name: &str, content: &str, live: bool) -> DevProcessRecord {
        DevProcessRecord {
            name: name.to_owned(),
            content: content.to_owned(),
            live,
            command: Some(format!("run {name}")),
            ..DevProcessRecord::default()
        }
    }

    #[test]
    fn processes_split_side_by_side_while_wide_enough_else_stacked() {
        assert_eq!(dev_split(81, 2), DevSplit::Columns);
        assert_eq!(dev_split(80, 2), DevSplit::Rows);
        assert_eq!(dev_split(30, 1), DevSplit::Rows);
        let area = Rect::new(2, 3, 101, 20);
        assert_eq!(dev_cells(area, 2), [Rect::new(2, 3, 50, 20), Rect::new(53, 3, 50, 20)]);
        assert_eq!(
            dev_cells(Rect::new(0, 0, 60, 21), 2),
            [Rect::new(0, 0, 60, 11), Rect::new(0, 11, 60, 10)]
        );
        // The PTYs take the smallest cell, less its header row.
        assert_eq!(dev_cell_size(101, 20, 2), (50, 19));
        assert_eq!(dev_cell_size(60, 21, 2), (60, 9));
        assert_eq!(dev_cell_size(60, 21, 1), (60, 20));
    }

    #[test]
    fn named_processes_render_in_cells_with_the_selected_one_marked() {
        let session = session(json!({}));
        let record = DevRecord {
            processes: Some(vec![process("api", "api-up", true), process("web", "", false)]),
            ..dev("", true, Some("s1"))
        };
        let props = DevPaneProps {
            session: Some(&session),
            dev: &record,
            selected_process: Some("web"),
        };
        let area = Rect::new(0, 0, 91, 4);
        let mut buf = Buffer::empty(area);
        render_dev_pane(&props, &mut buf, area);
        let line = |y: u16| -> String { (0..area.width).map(|x| buf[(x, y)].symbol()).collect::<String>() };
        assert_eq!(
            line(0),
            "  api · run api                        ● live│▸ web · run web                     ○ stopped"
        );
        assert!(line(1).starts_with("api-up"));
        assert!(line(1).contains("│Not running. r restarts it"));
        assert_eq!(selected_process_index(&record, Some("web")), 1);
        assert_eq!(selected_process_index(&record, Some("gone")), 0);
        let exited = DevProcessRecord {
            exit_code: Some(1),
            ..process("api", "boom", false)
        };
        assert!(row_text(&process_rows(&exited, false, 30, 2)[0]).ends_with("✗ exit 1"));
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
                selected_process: None,
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
                selected_process: None,
            },
            50,
            2,
        );
        assert!(row_text(&idle[0]).starts_with("Select a session from the sidebar."));
        assert!(row_text(&idle[0]).ends_with("○ idle"));
    }
}
