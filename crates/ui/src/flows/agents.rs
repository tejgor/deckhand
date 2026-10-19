//! Port of agentsFlow.tsx: U → Agents. Each agent's installed and latest version and its own updater (enter).
//! Updating never touches sessions: running ones keep their version until restarted (↑ in the sidebar). r looks the
//! latest up again.

use std::rc::Rc;

use indexmap::{IndexMap, IndexSet};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;

use deckhand_client::live::LiveClient;
use deckhand_core::display::DetailLine;
use deckhand_core::key::Input;
use deckhand_protocol::data::{AgentUpdateResult, AgentVersionInfo, AgentVersions, ProgramKey};

use crate::app::state::App;
use crate::flow::{Flow, FlowCx};
use crate::keymap::Keymap;

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
    let _ = (count, word);
    todo!("F1")
}

/// TS: agentsFlow.tsx rowStatus: `updating…`, or the update status, and its color.
pub fn row_status(info: Option<&AgentVersionInfo>, working: bool) -> (String, Color) {
    let _ = (info, working);
    todo!("F1")
}

/// TS: agentsFlow.tsx rowDescription: installed, latest and outdated sessions, in columns.
pub fn row_description(info: Option<&AgentVersionInfo>) -> String {
    let _ = info;
    todo!("F1")
}

/// TS: agentsFlow.tsx outcomeLines: an update's result and the tail of its output.
pub fn outcome_lines(outcome: &AgentUpdateOutcome) -> Vec<DetailLine> {
    let _ = outcome;
    todo!("F1")
}

/// TS: agentsFlow.tsx detailLines: the selected agent's details (`now`: ms since the epoch).
pub fn detail_lines(
    info: Option<&AgentVersionInfo>,
    program: ProgramKey,
    working: bool,
    outcome: Option<&AgentUpdateOutcome>,
    now: i64,
) -> Vec<DetailLine> {
    let _ = (info, program, working, outcome, now);
    todo!("F1")
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
    let _ = (props, buf, area);
    todo!("F1")
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
        let _ = (props, cx);
        todo!("F1")
    }

    /// TS: useAgentsFlow check: `agent-versions` with refresh (offline only leaves the latest unknown).
    pub fn check(&mut self, props: &AgentsFlowProps<'_>, cx: &mut FlowCx<App, Self>) {
        let _ = (props, cx);
        todo!("F1")
    }

    /// TS: useAgentsFlow update: runs the agent's updater; the outcome shows in the details box.
    pub fn update(&mut self, program: ProgramKey, props: &AgentsFlowProps<'_>, cx: &mut FlowCx<App, Self>) {
        let _ = (program, props, cx);
        todo!("F1")
    }
}

impl Flow<App> for AgentsFlow {
    type Props<'a> = AgentsFlowProps<'a>;

    fn handle_input(&mut self, props: &AgentsFlowProps<'_>, input: &Input, cx: &mut FlowCx<App, Self>) {
        let _ = (props, input, cx);
        todo!("F1")
    }

    fn render(&mut self, props: &AgentsFlowProps<'_>, buf: &mut Buffer, area: Rect) {
        let _ = (props, buf, area);
        todo!("F1")
    }
}
