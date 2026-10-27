//! G → Agents. Each agent's installed and latest version and its own updater (enter). Updating never touches sessions:
//! running ones keep their version until restarted (↑ in the sidebar). r looks the latest up again.

use std::rc::Rc;

use indexmap::{IndexMap, IndexSet};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use deckhand_client::live::LiveClient;
use deckhand_core::agent_versions::{AGENT_PROGRAMS, AgentUpdateStatus, agent_update_status};
use deckhand_core::agents::agent_spec;
use deckhand_core::display::{
    Color, DetailLine, HintPart, MenuItem, MenuPaneLine, RowStatus, THEME, compact_path, error_message,
    plain_terminal_text,
};
use deckhand_core::jsstr::{date, js_pad_end, js_trim_end};
use deckhand_core::key::Input;
use deckhand_core::sidebar_model::format_age;
use deckhand_protocol::data::{AgentUpdateResult, AgentVersionInfo, AgentVersions, ProgramKey};

use crate::app::state::{App, Mode};
use crate::flow::{Flow, FlowCx, FlowEffect};
use crate::keymap::Keymap;
use crate::panes::menu::{MenuPaneDetails, MenuPaneProps, render_menu_pane};

/// TS: agentsFlow.tsx AgentUpdateOutcome: an update's result, or why it could not run.
#[derive(Clone, Debug, PartialEq)]
pub enum AgentUpdateOutcome {
    Result(Box<AgentUpdateResult>),
    Error { program: ProgramKey, error: String },
}

/// TS: agentsFlow.tsx OUTPUT_LINES: the last lines of an update's output shown in the details box.
pub const OUTPUT_LINES: usize = 8;

/// TS: agentsFlow.tsx plural (`1 session`, `2 sessions`).
pub fn plural(count: i64, word: &str) -> String {
    format!("{count} {word}{}", if count == 1 { "" } else { "s" })
}

/// The agent's version record (TS `versions?.[program]`).
fn info_of(versions: Option<&AgentVersions>, program: ProgramKey) -> Option<&AgentVersionInfo> {
    versions.map(|versions| match program {
        ProgramKey::Claude => &versions.claude,
        ProgramKey::Pi => &versions.pi,
        ProgramKey::Codex => &versions.codex,
    })
}

/// A TS truthy string field (`undefined` and `''` are not).
fn present(value: Option<&String>) -> Option<&str> {
    value.map(String::as_str).filter(|text| !text.is_empty())
}

/// TS: agentsFlow.tsx rowStatus: `updating…`, or the update status, and its color (an Ink color name, as the menu
/// rows take it).
pub fn row_status(info: Option<&AgentVersionInfo>, working: bool) -> (String, Color) {
    if working {
        return ("updating…".to_owned(), THEME.warn);
    }
    let status = agent_update_status(info);
    let color = match status {
        AgentUpdateStatus::UpdateAvailable => THEME.warn,
        AgentUpdateStatus::UpToDate => THEME.success,
        AgentUpdateStatus::NotInstalled | AgentUpdateStatus::LatestUnknown => THEME.muted,
    };
    (status.as_str().to_owned(), color)
}

/// TS: agentsFlow.tsx rowDescription: installed, latest and outdated sessions, in columns.
pub fn row_description(info: Option<&AgentVersionInfo>) -> String {
    let has_path = info.is_some_and(|info| present(info.path.as_ref()).is_some());
    // `??`: only a missing version falls back (an empty one stays empty).
    let installed = match info.and_then(|info| info.installed.as_deref()) {
        Some(installed) => installed,
        None if has_path => "unknown",
        None => "—",
    };
    let latest = format!("latest {}", info.and_then(|info| info.latest.as_deref()).unwrap_or("?"));
    let outdated = match info {
        Some(info) if info.outdated != 0 => format!("{} on an older version", plural(info.outdated, "session")),
        _ => String::new(),
    };
    let row = format!(
        "{}{}{outdated}",
        js_pad_end(installed, 10, " "),
        js_pad_end(&latest, 16, " ")
    );
    js_trim_end(&row).to_owned()
}

/// A detail line with only text and color.
fn line(text: String, color: Option<Color>) -> DetailLine {
    DetailLine {
        text,
        color,
        ..DetailLine::default()
    }
}

/// A detail line that is cut rather than wrapped.
fn nowrap(text: String) -> DetailLine {
    DetailLine {
        text,
        nowrap: Some(true),
        ..DetailLine::default()
    }
}

/// `exitCode ?? 'none'`.
fn exit_text(result: &AgentUpdateResult) -> String {
    result
        .exit_code
        .map_or_else(|| "none".to_owned(), |code| code.to_string())
}

/// The version an update installed, when it differs from the one before (`after && before !== after`).
fn updated_to(result: &AgentUpdateResult) -> Option<&str> {
    present(result.after.as_ref()).filter(|after| result.before.as_deref() != Some(after))
}

/// TS: agentsFlow.tsx outcomeLines: an update's result and the tail of its output.
pub fn outcome_lines(outcome: &AgentUpdateOutcome) -> Vec<DetailLine> {
    let result = match outcome {
        AgentUpdateOutcome::Error { error, .. } => {
            return vec![line(format!("Update failed: {error}"), Some(THEME.error))];
        }
        AgentUpdateOutcome::Result(result) => result,
    };
    let text = plain_terminal_text(&result.output);
    let lines: Vec<&str> = text
        .split('\n')
        .map(js_trim_end)
        .filter(|line| !line.is_empty())
        .collect();
    let output = &lines[lines.len().saturating_sub(OUTPUT_LINES)..];
    let head = if !result.ok {
        line(
            format!("{} failed (exit {}):", result.command, exit_text(result)),
            Some(THEME.error),
        )
    } else if let Some(after) = updated_to(result) {
        line(
            format!(
                "Updated {} → {after}. Running sessions keep their version until restarted.",
                result.before.as_deref().unwrap_or("?")
            ),
            Some(THEME.success),
        )
    } else {
        line(
            format!(
                "Nothing new installed ({}).",
                result.after.as_deref().unwrap_or("version unknown")
            ),
            Some(THEME.success),
        )
    };
    let mut all = vec![head];
    all.extend(output.iter().map(|text| DetailLine {
        text: (*text).to_owned(),
        nowrap: Some(true),
        color: result.ok.then_some(THEME.muted),
        parts: None,
    }));
    all
}

/// `<program> <update args>` (the agent's own update command).
fn update_command(program: ProgramKey) -> String {
    format!("{program} {}", agent_spec(program).version.update_args.join(" "))
}

/// TS: agentsFlow.tsx detailLines: the selected agent's details (`now`: ms since the epoch).
pub fn detail_lines(
    info: Option<&AgentVersionInfo>,
    program: ProgramKey,
    working: bool,
    outcome: Option<&AgentUpdateOutcome>,
    now: i64,
) -> Vec<DetailLine> {
    let spec = agent_spec(program);
    // An unreadable time is NaN in JS, which formatAge reads as `now`.
    let age = present(info.and_then(|info| info.checked_at.as_ref()))
        .map(|checked_at| format_age(date::parse(checked_at).map_or(f64::NAN, |at| now.saturating_sub(at) as f64)));
    let checked = match age.as_deref() {
        Some("now") => " · checked just now".to_owned(),
        Some(age) => format!(" · checked {age} ago"),
        None => String::new(),
    };
    let path = info.and_then(|info| present(info.path.as_ref()));
    // A running update or its result first: the box is cut from the bottom when the pane is short.
    let mut lines: Vec<DetailLine> = if working {
        vec![line(
            format!("Running {}… sessions keep running.", update_command(program)),
            Some(THEME.warn),
        )]
    } else if let Some(outcome) = outcome {
        outcome_lines(outcome)
    } else if path.is_none() {
        vec![line(
            "Not installed. Run `deckhand setup` to install it.".to_owned(),
            Some(THEME.muted),
        )]
    } else {
        Vec::new()
    };
    let installed = info
        .and_then(|info| info.installed.as_deref())
        .unwrap_or(if path.is_some() {
            "version unknown"
        } else {
            "not installed"
        });
    let installed_path = path.map_or_else(String::new, |path| format!("  {}", compact_path(path, 60)));
    let running = info.map_or(0, |info| info.running);
    let sessions = if running != 0 {
        let outdated = info.map_or(0, |info| info.outdated);
        let older = if outdated != 0 {
            format!(" · {outdated} on an older version (restart to update)")
        } else {
            String::new()
        };
        format!("{}{older}", plural(running, "running session"))
    } else {
        "none running".to_owned()
    };
    lines.push(nowrap(format!("Installed  {installed}{installed_path}")));
    lines.push(nowrap(format!(
        "Latest     {}  npm {}{checked}",
        info.and_then(|info| info.latest.as_deref()).unwrap_or("unknown"),
        spec.version.npm_package
    )));
    lines.push(nowrap(format!("Update     {}", update_command(program))));
    lines.push(nowrap(format!("Sessions   {sessions}")));
    if let Some(error) = info.and_then(|info| present(info.error.as_ref())) {
        lines.push(line(error.to_owned(), Some(THEME.muted)));
    }
    lines
}

/// TS: agentsFlow.tsx AgentsPaneProps (width and height are the area's).
#[derive(Clone, Copy, Debug)]
pub struct AgentsPaneProps<'a> {
    pub versions: Option<&'a AgentVersions>,
    pub selected: usize,
    pub checking: bool,
    /// Agents whose update is running (this UI's requests or the daemon's `updating`).
    pub working: &'a IndexSet<ProgramKey>,
    pub outcomes: &'a IndexMap<ProgramKey, AgentUpdateOutcome>,
    /// The agent whose update waits for confirmation (it has running sessions).
    pub confirming: Option<ProgramKey>,
    pub now: i64,
}

/// TS: agentsFlow.tsx AgentsPane.
pub fn render_agents_pane(props: &AgentsPaneProps<'_>, buf: &mut Buffer, area: Rect) {
    // This UI's request or the daemon's `updating`.
    let is_working = |program: ProgramKey| {
        props.working.contains(&program)
            || info_of(props.versions, program).is_some_and(|info| info.updating == Some(true))
    };
    let items: Vec<MenuItem> = AGENT_PROGRAMS
        .iter()
        .map(|&program| {
            let info = info_of(props.versions, program);
            let (text, color) = row_status(info, is_working(program));
            MenuItem {
                key: program.as_str().to_owned(),
                label: agent_spec(program).label.to_owned(),
                description: Some(row_description(info)),
                status: Some(RowStatus {
                    text,
                    color: Some(color),
                }),
                ..MenuItem::default()
            }
        })
        .collect();
    let program = AGENT_PROGRAMS.get(props.selected).copied().unwrap_or(AGENT_PROGRAMS[0]);
    let info = info_of(props.versions, program);
    let details = match props.confirming {
        Some(confirming) => {
            let label = agent_spec(confirming).label;
            let running = info_of(props.versions, confirming).map_or(0, |info| info.running);
            MenuPaneDetails {
                title: format!("Update {label}?"),
                lines: vec![
                    line(
                        format!(
                            "{} keep their current version until restarted.",
                            plural(running, &format!("running {label} session"))
                        ),
                        None,
                    ),
                    line(format!("Runs {}.", update_command(confirming)), Some(THEME.muted)),
                ],
            }
        }
        None => MenuPaneDetails {
            title: format!(
                "{} · {}",
                agent_spec(program).label,
                row_status(info, is_working(program)).0
            ),
            lines: detail_lines(
                info,
                program,
                is_working(program),
                props.outcomes.get(&program),
                props.now,
            ),
        },
    };
    let subtitle = [if props.checking {
        MenuPaneLine {
            text: "Checking the latest releases…".to_owned(),
            color: Some(THEME.warn),
        }
    } else {
        MenuPaneLine {
            text: "Running sessions keep their version until restarted (↑ in the sidebar).".to_owned(),
            color: None,
        }
    }];
    let hint: Vec<HintPart> = if props.confirming.is_some() {
        vec![HintPart::from("enter update"), HintPart::from("esc cancel")]
    } else {
        vec![
            HintPart::from("enter update"),
            HintPart {
                text: "r re-check".to_owned(),
                short: None,
                drop: Some(2),
            },
            HintPart::from("esc back"),
        ]
    };
    render_menu_pane(
        &MenuPaneProps {
            title: "Agents",
            subtitle: &subtitle,
            items: &items,
            selected: props.selected.min(items.len() - 1),
            details: Some(&details),
            empty: None,
            hint: &hint,
            label_width: Some(8),
        },
        buf,
        area,
    );
}

/// TS: agentsFlow.tsx AgentsFlowOptions (values; `setVersions`, `setMode` and `setStatusMessage` are effects).
#[derive(Clone, Copy, Debug)]
pub struct AgentsFlowProps<'a> {
    pub client: Option<&'a Rc<LiveClient>>,
    pub versions: Option<&'a AgentVersions>,
}

/// TS: agentsFlow.tsx useAgentsFlow's state (AgentsFlow).
#[derive(Debug, Default)]
pub struct AgentsFlow {
    pub selected: usize,
    pub checking: bool,
    pub confirming: Option<ProgramKey>,
    pub working: IndexSet<ProgramKey>,
    pub outcomes: IndexMap<ProgramKey, AgentUpdateOutcome>,
}

/// The Agents screen's keys.
pub static KEYMAP: Keymap = &[];

impl AgentsFlow {
    /// TS: AgentsFlow.open (U): show the screen and look the latest releases up again.
    pub fn open(&mut self, props: &AgentsFlowProps<'_>, cx: &mut FlowCx<App, Self>) {
        self.confirming = None;
        cx.emit(FlowEffect::SetMode(Mode::Agents));
        self.check(props, cx);
    }

    /// TS: useAgentsFlow check: `agent-versions` with refresh (offline only leaves the latest unknown).
    pub fn check(&mut self, props: &AgentsFlowProps<'_>, cx: &mut FlowCx<App, Self>) {
        let Some(client) = props.client else { return };
        self.checking = true;
        // Offline or npm missing only leaves the latest unknown; nothing to report.
        cx.spawn(client.agent_versions(Some(true)), |flow: &mut Self, result, cx| {
            if let Ok(versions) = result {
                cx.emit(FlowEffect::Versions(Box::new(versions)));
            }
            flow.checking = false;
        });
    }

    /// TS: useAgentsFlow update: runs the agent's updater; the outcome shows in the details box.
    pub fn update(&mut self, program: ProgramKey, props: &AgentsFlowProps<'_>, cx: &mut FlowCx<App, Self>) {
        let Some(client) = props.client else { return };
        let label = agent_spec(program).label;
        self.working.insert(program);
        self.outcomes.shift_remove(&program);
        cx.spawn(client.update_agent(program), move |flow: &mut Self, result, cx| {
            match result {
                Ok(result) => {
                    cx.emit(FlowEffect::Versions(Box::new(result.versions.clone())));
                    let message = if !result.ok {
                        format!(
                            "{label} update failed (exit {}); U shows the output",
                            exit_text(&result)
                        )
                    } else if let Some(after) = updated_to(&result) {
                        format!("{label} updated to {after}")
                    } else {
                        format!("{label}: nothing new installed")
                    };
                    flow.outcomes
                        .insert(program, AgentUpdateOutcome::Result(Box::new(result)));
                    cx.set_status(message);
                }
                Err(error) => {
                    flow.outcomes.insert(
                        program,
                        AgentUpdateOutcome::Error {
                            program,
                            error: error_message(&error),
                        },
                    );
                    cx.set_status(format!("{label} update failed; U shows why"));
                }
            }
            flow.working.shift_remove(&program);
        });
    }
}

impl Flow<App> for AgentsFlow {
    type Props<'a> = AgentsFlowProps<'a>;

    fn handle_input(&mut self, props: &AgentsFlowProps<'_>, input: &Input, cx: &mut FlowCx<App, Self>) {
        let key = &input.key;
        let text = input.input.as_str();
        let program = AGENT_PROGRAMS.get(self.selected).copied().unwrap_or(AGENT_PROGRAMS[0]);
        if let Some(confirming) = self.confirming {
            if key.return_ || text == "y" {
                self.confirming = None;
                self.update(confirming, props, cx);
            } else if key.escape || text == "n" {
                self.confirming = None;
            }
            return;
        }
        if key.escape {
            cx.emit(FlowEffect::SetMode(Mode::Browse));
            return;
        }
        if key.up_arrow || text == "k" {
            self.selected = self.selected.saturating_sub(1);
            return;
        }
        if key.down_arrow || text == "j" {
            self.selected = (self.selected + 1).min(AGENT_PROGRAMS.len() - 1);
            return;
        }
        if text == "r" {
            self.check(props, cx);
            return;
        }
        if key.return_ {
            // Installing is deckhand setup's job (the details say so); one update at a time per agent.
            let Some(info) = info_of(props.versions, program).filter(|info| present(info.path.as_ref()).is_some())
            else {
                return;
            };
            if self.working.contains(&program) || info.updating == Some(true) {
                return;
            }
            if info.running > 0 {
                self.confirming = Some(program);
            } else {
                self.update(program, props, cx);
            }
        }
    }

    fn render(&mut self, props: &AgentsFlowProps<'_>, buf: &mut Buffer, area: Rect) {
        render_agents_pane(
            &AgentsPaneProps {
                versions: props.versions,
                selected: self.selected,
                checking: self.checking,
                working: &self.working,
                outcomes: &self.outcomes,
                confirming: self.confirming,
                now: date::now_ms(),
            },
            buf,
            area,
        );
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::UnixStream;
    use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};

    use deckhand_client::live::LiveClientHandlers;
    use deckhand_core::key::Key;

    use super::*;
    use crate::app::state::AppProps;
    use crate::app::test_support::app;

    const NOW: i64 = 1_791_201_600_000; // 2026-10-05T12:00:00.000Z

    fn ago(minutes: i64) -> String {
        date::to_iso_string(NOW - minutes * 60_000).expect("iso")
    }

    fn info(program: &str, fields: Value) -> AgentVersionInfo {
        let mut value = json!({"program": program, "running": 0, "outdated": 0});
        if let (Some(base), Some(extra)) = (value.as_object_mut(), fields.as_object()) {
            base.extend(extra.clone());
        }
        serde_json::from_value(value).expect("info")
    }

    fn versions(claude: Value, pi: Value, codex: Value) -> AgentVersions {
        AgentVersions {
            claude: info("claude", claude),
            pi: info("pi", pi),
            codex: info("codex", codex),
        }
    }

    fn fixture() -> AgentVersions {
        versions(
            json!({"installed": "2.1.287", "latest": "2.1.291", "path": "/Users/me/.local/bin/claude", "checkedAt": ago(3), "running": 3, "outdated": 1}),
            json!({"installed": "1.0.2", "path": "/usr/local/bin/pi", "checkedAt": ago(3), "error": "latest unknown: npm view timed out"}),
            json!({"installed": "0.160.1", "latest": "0.160.1", "path": "/usr/local/bin/codex", "checkedAt": ago(3)}),
        )
    }

    fn result(fields: Value, versions: &AgentVersions) -> AgentUpdateOutcome {
        let mut value = json!({"program": "claude", "ok": true, "exitCode": 0, "command": "claude update", "output": "", "versions": versions});
        if let (Some(base), Some(extra)) = (value.as_object_mut(), fields.as_object()) {
            base.extend(extra.clone());
        }
        AgentUpdateOutcome::Result(Box::new(serde_json::from_value(value).expect("result")))
    }

    struct Pane<'a> {
        versions: Option<&'a AgentVersions>,
        selected: usize,
        checking: bool,
        working: IndexSet<ProgramKey>,
        outcomes: IndexMap<ProgramKey, AgentUpdateOutcome>,
        confirming: Option<ProgramKey>,
    }

    impl<'a> Pane<'a> {
        fn new(versions: &'a AgentVersions) -> Self {
            Pane {
                versions: Some(versions),
                selected: 0,
                checking: false,
                working: IndexSet::new(),
                outcomes: IndexMap::new(),
                confirming: None,
            }
        }

        /// The screen as text (TS `plain(renderToString(...))`).
        fn render(&self, width: u16, height: u16) -> String {
            let area = Rect::new(0, 0, width, height);
            let mut buf = Buffer::empty(area);
            render_agents_pane(
                &AgentsPaneProps {
                    versions: self.versions,
                    selected: self.selected,
                    checking: self.checking,
                    working: &self.working,
                    outcomes: &self.outcomes,
                    confirming: self.confirming,
                    now: NOW,
                },
                &mut buf,
                area,
            );
            (0..height)
                .map(|y| {
                    (0..width)
                        .map(|x| buf[(x, y)].symbol().to_owned())
                        .collect::<String>()
                        .trim_end()
                        .to_owned()
                })
                .collect::<Vec<_>>()
                .join("\n")
        }
    }

    fn assert_has(screen: &str, texts: &[&str]) {
        for text in texts {
            assert!(screen.contains(text), "{text}\n{screen}");
        }
    }

    #[test]
    fn agents_screen_rows_details_confirmation_results() {
        let fixture = fixture();
        let mut pane = Pane::new(&fixture);
        assert_has(
            &pane.render(100, 22),
            &[
                "Agents",
                "Running sessions keep their version until restarted",
                "Claude    2.1.287   latest 2.1.291  1 session on an older version",
                "update available",
                "Pi        1.0.2     latest ?",
                "latest unknown",
                "Codex     0.160.1   latest 0.160.1",
                "up to date",
                "Claude · update available",
                "Installed  2.1.287  /Users/me/.local/bin/claude",
                "Latest     2.1.291  npm @anthropic-ai/claude-code · checked 3m ago",
                "Update     claude update",
                "3 running sessions · 1 on an older version (restart to update)",
                "enter update",
                "r re-check",
                "esc back",
            ],
        );
        pane.selected = 1;
        assert!(pane.render(100, 22).contains("latest unknown: npm view timed out"));
        let missing = versions(json!({}), json!({}), json!({}));
        pane.versions = Some(&missing);
        pane.selected = 0;
        assert!(pane.render(100, 22).contains("not installed"));
        pane.selected = 1;
        assert!(pane.render(100, 22).contains("Run `deckhand setup` to install it."));
        pane.versions = Some(&fixture);
        pane.selected = 0;
        pane.checking = true;
        assert!(pane.render(100, 22).contains("Checking the latest releases…"));
        pane.checking = false;
        pane.confirming = Some(ProgramKey::Claude);
        assert_has(
            &pane.render(100, 22),
            &[
                "Update Claude?",
                "3 running Claude sessions keep their current version until restarted.",
                "Runs claude update.",
                "enter update • esc cancel",
            ],
        );
        pane.confirming = None;
        pane.working.insert(ProgramKey::Claude);
        let working = pane.render(100, 22);
        assert!(working.contains("Running claude update… sessions keep running."));
        assert!(working.contains("updating…"));
        pane.working.clear();
        pane.outcomes.insert(
            ProgramKey::Claude,
            result(
                json!({"output": "Checking…\nUpdated to 2.1.291", "before": "2.1.287", "after": "2.1.291"}),
                &fixture,
            ),
        );
        let done = pane.render(100, 22);
        assert!(
            done.contains("Updated 2.1.287 → 2.1.291. Running sessions keep their version until restarted."),
            "{done}"
        );
        pane.outcomes.insert(
            ProgramKey::Claude,
            result(
                json!({"ok": false, "exitCode": 1, "output": "line 1\nError: permission denied", "before": "2.1.287", "after": "2.1.287"}),
                &fixture,
            ),
        );
        assert_has(
            &pane.render(100, 22),
            &["claude update failed (exit 1):", "line 1", "Error: permission denied"],
        );
    }

    #[test]
    fn the_daemons_updating_flag_shows_as_working() {
        let fixture = versions(
            json!({"installed": "1.0.0", "path": "/bin/claude", "updating": true}),
            json!({}),
            json!({}),
        );
        let pane = Pane::new(&fixture);
        let screen = pane.render(100, 22);
        assert_has(
            &screen,
            &[
                "updating…",
                "Claude · updating…",
                "Running claude update… sessions keep running.",
            ],
        );
    }

    #[test]
    fn plural_and_rows() {
        assert_eq!(plural(1, "session"), "1 session");
        assert_eq!(plural(0, "session"), "0 sessions");
        assert_eq!(plural(2, "running session"), "2 running sessions");
        let missing = info("pi", json!({}));
        assert_eq!(row_description(Some(&missing)), "—         latest ?");
        assert_eq!(row_description(None), "—         latest ?");
        let no_version = info("pi", json!({"path": "/bin/pi"}));
        assert_eq!(row_description(Some(&no_version)), "unknown   latest ?");
        let outdated = info(
            "claude",
            json!({"installed": "1.0.0", "latest": "1.0.1", "outdated": 2}),
        );
        assert_eq!(
            row_description(Some(&outdated)),
            "1.0.0     latest 1.0.1    2 sessions on an older version"
        );
        assert_eq!(row_status(None, true), ("updating…".to_owned(), THEME.warn));
        assert_eq!(
            row_status(Some(&outdated), false),
            ("update available".to_owned(), THEME.warn)
        );
        assert_eq!(row_status(None, false), ("not installed".to_owned(), THEME.muted));
        let current = info("claude", json!({"installed": "1.0.1", "latest": "1.0.1"}));
        assert_eq!(
            row_status(Some(&current), false),
            ("up to date".to_owned(), THEME.success)
        );
    }

    #[test]
    fn outcomes_show_the_tail_of_the_output() {
        let empty = versions(json!({}), json!({}), json!({}));
        let output: String = (1..=12).map(|n| format!("line {n}  \r\n")).collect();
        let ok = result(json!({"output": output, "before": "1.0.0", "after": "1.0.0"}), &empty);
        let lines = outcome_lines(&ok);
        assert_eq!(lines.len(), 1 + OUTPUT_LINES);
        assert_eq!(lines[0].text, "Nothing new installed (1.0.0).");
        assert_eq!(lines[1].text, "line 5");
        assert_eq!(lines[8].text, "line 12");
        assert_eq!(lines[1].color, Some(THEME.muted));
        assert_eq!(lines[1].nowrap, Some(true));
        let failed = result(json!({"ok": false, "exitCode": null, "output": "boom\n\n"}), &empty);
        let lines = outcome_lines(&failed);
        assert_eq!(lines[0].text, "claude update failed (exit none):");
        assert_eq!(lines[0].color, Some(THEME.error));
        assert_eq!(lines[1].text, "boom");
        assert_eq!(lines[1].color, None);
        let unknown = result(json!({"output": ""}), &empty);
        assert_eq!(
            outcome_lines(&unknown)[0].text,
            "Nothing new installed (version unknown)."
        );
        let error = AgentUpdateOutcome::Error {
            program: ProgramKey::Pi,
            error: "closed".to_owned(),
        };
        assert_eq!(
            outcome_lines(&error),
            vec![line("Update failed: closed".to_owned(), Some(THEME.error))]
        );
    }

    #[test]
    fn checked_age_reads_unparsable_times_as_just_now() {
        let odd = info(
            "claude",
            json!({"installed": "1.0.0", "path": "/bin/claude", "checkedAt": "not a date"}),
        );
        let lines = detail_lines(Some(&odd), ProgramKey::Claude, false, None, NOW);
        assert_eq!(
            lines[1].text,
            "Latest     unknown  npm @anthropic-ai/claude-code · checked just now"
        );
        assert_eq!(lines[0].text, "Installed  1.0.0  /bin/claude");
        assert_eq!(lines[3].text, "Sessions   none running");
    }

    fn key(setup: impl FnOnce(&mut Key)) -> Input {
        let mut key = Key::NONE;
        setup(&mut key);
        Input::key("", key)
    }

    fn press(app: &mut App, input: &Input) {
        app.agents_flow_call(|flow, props, cx| flow.handle_input(props, input, cx));
    }

    #[test]
    fn keys_move_confirm_and_leave() {
        let (mut app, _rx) = app(AppProps::default());
        let mut fixture = fixture();
        fixture.codex.running = 2;
        app.data.agent_versions = Some(fixture);
        app.set_mode(Mode::Agents);
        press(&mut app, &Input::text("j"));
        press(&mut app, &Input::text("j"));
        press(&mut app, &Input::text("j"));
        assert_eq!(app.flows.agents.selected, 2);
        press(&mut app, &key(|key| key.up_arrow = true));
        press(&mut app, &Input::text("k"));
        press(&mut app, &Input::text("k"));
        assert_eq!(app.flows.agents.selected, 0);
        // Pi has a path but no running sessions and, without a client, nothing to start.
        press(&mut app, &key(|key| key.down_arrow = true));
        press(&mut app, &key(|key| key.return_ = true));
        assert_eq!(app.flows.agents.confirming, None);
        assert!(app.flows.agents.working.is_empty());
        // Claude has running sessions: enter asks first; n and esc cancel, other keys do nothing.
        press(&mut app, &Input::text("k"));
        press(&mut app, &key(|key| key.return_ = true));
        assert_eq!(app.flows.agents.confirming, Some(ProgramKey::Claude));
        press(&mut app, &Input::text("j"));
        press(&mut app, &Input::text("r"));
        assert_eq!(app.flows.agents.confirming, Some(ProgramKey::Claude));
        assert_eq!(app.flows.agents.selected, 0);
        press(&mut app, &Input::text("n"));
        assert_eq!(app.flows.agents.confirming, None);
        press(&mut app, &key(|key| key.return_ = true));
        press(&mut app, &key(|key| key.escape = true));
        assert_eq!(app.flows.agents.confirming, None);
        assert_eq!(app.mode, Mode::Agents);
        press(&mut app, &key(|key| key.return_ = true));
        press(&mut app, &Input::text("y"));
        assert_eq!(app.flows.agents.confirming, None);
        // Not installed or already updating: enter does nothing.
        app.data.agent_versions = Some(versions(
            json!({}),
            json!({"installed": "1.0.0", "path": "/bin/pi", "running": 4}),
            json!({"installed": "1.0.0", "path": "/bin/codex", "updating": true}),
        ));
        press(&mut app, &key(|key| key.return_ = true));
        assert_eq!(app.flows.agents.confirming, None);
        app.flows.agents.selected = 1;
        press(&mut app, &key(|key| key.return_ = true));
        assert_eq!(app.flows.agents.confirming, Some(ProgramKey::Pi));
        press(&mut app, &key(|key| key.escape = true));
        app.flows.agents.selected = 2;
        press(&mut app, &key(|key| key.return_ = true));
        assert_eq!(app.flows.agents.confirming, None);
        press(&mut app, &key(|key| key.escape = true));
        assert_eq!(app.mode, Mode::Browse);
    }

    struct Daemon {
        lines: BufReader<OwnedReadHalf>,
        writer: OwnedWriteHalf,
    }

    impl Daemon {
        async fn request(&mut self) -> Value {
            let mut line = String::new();
            self.lines.read_line(&mut line).await.expect("read");
            serde_json::from_str(&line).expect("json")
        }

        async fn respond(&mut self, request: &Value, body: Value) {
            let mut message = json!({"type": "response", "requestId": request["requestId"]});
            if let (Some(message), Some(body)) = (message.as_object_mut(), body.as_object()) {
                message.extend(body.clone());
            }
            self.writer
                .write_all(format!("{message}\n").as_bytes())
                .await
                .expect("write");
        }
    }

    fn run(test: impl std::future::Future<Output = ()>) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        tokio::task::LocalSet::new().block_on(&runtime, test);
    }

    fn connected(app: &mut App) -> Daemon {
        let (client, server) = UnixStream::pair().expect("pair");
        let (read, writer) = server.into_split();
        app.connection.client = Some(Rc::new(LiveClient::new(client, LiveClientHandlers::default())));
        Daemon {
            lines: BufReader::new(read),
            writer,
        }
    }

    type Updates = tokio::sync::mpsc::UnboundedReceiver<crate::app::event_loop::Update<App>>;

    async fn apply(app: &mut App, rx: &mut Updates) {
        let update = rx.recv().await.expect("update");
        update(app);
    }

    #[test]
    fn open_looks_the_latest_up_again_and_keeps_the_versions() {
        run(async {
            let (mut app, mut rx) = app(AppProps::default());
            let mut daemon = connected(&mut app);
            app.flows.agents.confirming = Some(ProgramKey::Pi);
            app.agents_flow_call(|flow, props, cx| flow.open(props, cx));
            assert_eq!(app.mode, Mode::Agents);
            assert_eq!(app.flows.agents.confirming, None);
            assert!(app.flows.agents.checking);
            let request = daemon.request().await;
            assert_eq!(request["type"], "agent-versions");
            assert_eq!(request["refresh"], true);
            let latest = fixture();
            daemon.respond(&request, json!({"ok": true, "data": latest})).await;
            apply(&mut app, &mut rx).await;
            assert!(!app.flows.agents.checking);
            assert_eq!(app.data.agent_versions, Some(latest));
            // Offline: the failure is swallowed, checking ends, the versions stay.
            press(&mut app, &Input::text("r"));
            assert!(app.flows.agents.checking);
            let request = daemon.request().await;
            daemon.respond(&request, json!({"ok": false, "error": "offline"})).await;
            apply(&mut app, &mut rx).await;
            assert!(!app.flows.agents.checking);
            assert!(app.data.agent_versions.is_some());
        });
    }

    #[test]
    fn updating_runs_the_agents_updater_and_reports_the_outcome() {
        run(async {
            let (mut app, mut rx) = app(AppProps::default());
            let mut daemon = connected(&mut app);
            app.data.agent_versions = Some(fixture());
            app.set_mode(Mode::Agents);
            app.flows.agents.selected = 2;
            app.flows.agents.outcomes.insert(
                ProgramKey::Codex,
                AgentUpdateOutcome::Error {
                    program: ProgramKey::Codex,
                    error: "old".to_owned(),
                },
            );
            press(&mut app, &key(|key| key.return_ = true));
            assert!(app.flows.agents.working.contains(&ProgramKey::Codex));
            assert!(app.flows.agents.outcomes.is_empty());
            let request = daemon.request().await;
            assert_eq!(request["type"], "update-agent");
            assert_eq!(request["program"], "codex");
            let after = versions(
                json!({}),
                json!({}),
                json!({"installed": "0.161.0", "latest": "0.161.0", "path": "/usr/local/bin/codex"}),
            );
            let body = json!({"program": "codex", "ok": true, "exitCode": 0, "command": "codex update", "output": "done", "before": "0.160.1", "after": "0.161.0", "versions": after});
            daemon.respond(&request, json!({"ok": true, "data": body})).await;
            apply(&mut app, &mut rx).await;
            assert!(app.flows.agents.working.is_empty());
            assert_eq!(app.data.agent_versions, Some(after.clone()));
            assert_eq!(app.messages.status_message.as_deref(), Some("Codex updated to 0.161.0"));
            assert!(matches!(
                app.flows.agents.outcomes.get(&ProgramKey::Codex),
                Some(AgentUpdateOutcome::Result(_))
            ));

            // A failed update points at U; a refused one says why.
            press(&mut app, &key(|key| key.return_ = true));
            let request = daemon.request().await;
            let body = json!({"program": "codex", "ok": false, "exitCode": 1, "command": "codex update", "output": "no", "versions": after});
            daemon.respond(&request, json!({"ok": true, "data": body})).await;
            apply(&mut app, &mut rx).await;
            assert_eq!(
                app.messages.status_message.as_deref(),
                Some("Codex update failed (exit 1); U shows the output")
            );
            press(&mut app, &key(|key| key.return_ = true));
            let request = daemon.request().await;
            let body = json!({"program": "codex", "ok": true, "exitCode": 0, "command": "codex update", "output": "", "before": "0.161.0", "after": "0.161.0", "versions": after});
            daemon.respond(&request, json!({"ok": true, "data": body})).await;
            apply(&mut app, &mut rx).await;
            assert_eq!(
                app.messages.status_message.as_deref(),
                Some("Codex: nothing new installed")
            );
            press(&mut app, &key(|key| key.return_ = true));
            let request = daemon.request().await;
            daemon
                .respond(&request, json!({"ok": false, "error": "daemon busy"}))
                .await;
            apply(&mut app, &mut rx).await;
            assert_eq!(
                app.messages.status_message.as_deref(),
                Some("Codex update failed; U shows why")
            );
            assert_eq!(
                app.flows.agents.outcomes.get(&ProgramKey::Codex),
                Some(&AgentUpdateOutcome::Error {
                    program: ProgramKey::Codex,
                    error: "daemon busy".to_owned()
                })
            );
            assert!(app.flows.agents.working.is_empty());
        });
    }
}
