//! Port of settingsFlow.tsx: C → Settings. A grid of every setting with one column per layer; the cursor is a cell
//! (↑↓ setting, ←→ or Tab layer) and edits save immediately into that cell's layer through save-config's
//! revision-checked writes. E opens the raw JSON of the selected column's file and returns here; T reviews trust and
//! returns here. Saving never runs anything.

use std::rc::Rc;

use indexmap::IndexMap;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use serde_json::Value;

use deckhand_client::live::LiveClient;
use deckhand_core::display::DetailLine;
use deckhand_core::key::Input;
use deckhand_core::settings_model::{SettingChange, SettingId};
use deckhand_core::text_editor::EditorState;
use deckhand_protocol::data::{ConfigTargetKind, ProjectConfigDocument, SaveTrust, SettingsInfo, WorktreeCandidates};

use crate::app::state::{App, Mode};
use crate::flow::{Flow, FlowCx};
use crate::keymap::Keymap;
use crate::panes::settings::{LinksState, Notice, SettingsEdit, SettingsScroll, SettingsView};

/// TS: settingsFlow.tsx SettingsFlowMode.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SettingsFlowMode {
    Settings,
    EditProject,
    DiscardProject,
}

impl SettingsFlowMode {
    /// The app mode.
    pub fn mode(self) -> Mode {
        match self {
            SettingsFlowMode::Settings => Mode::Settings,
            SettingsFlowMode::EditProject => Mode::EditProject,
            SettingsFlowMode::DiscardProject => Mode::DiscardProject,
        }
    }
}

/// TS: settingsFlow.tsx isSettingsFlowMode.
pub fn is_settings_flow_mode(mode: Mode) -> bool {
    matches!(mode, Mode::Settings | Mode::EditProject | Mode::DiscardProject)
}

/// TS: settingsFlow.tsx SIZE_CHUNK: link candidate sizes are asked for this many paths at a time.
pub const SIZE_CHUNK: usize = 8;

/// TS: settingsFlow.tsx MAX_TEXT (8192 for the others).
pub fn max_text(id: SettingId) -> usize {
    let _ = id;
    todo!("F4")
}

/// TS: settingsFlow.tsx display: a value as the status says it (`a, b` / `none`, `on`/`off`, the string).
pub fn display(value: Option<&Value>) -> String {
    let _ = value;
    todo!("F4")
}

/// TS: settingsFlow.tsx capitalize.
pub fn capitalize(text: &str) -> String {
    let _ = text;
    todo!("F4")
}

/// TS: useSettingsFlow save's `fail`: where a failure is reported.
pub type SaveFail = Box<dyn FnOnce(&mut SettingsFlow, String)>;
/// TS: useSettingsFlow save's `after`: runs with the reloaded info.
pub type SaveAfter = Box<dyn FnOnce(&mut SettingsFlow, &SettingsInfo)>;
/// TS: useSettingsFlow load's `after`.
pub type LoadAfter = Box<dyn FnOnce(&mut SettingsFlow, &SettingsInfo, &mut FlowCx<App, SettingsFlow>)>;

/// TS: useSettingsFlow save's options.
#[derive(Default)]
pub struct SaveOptions {
    pub cleared: bool,
    pub extra: String,
    /// The status once saved, instead of savedMessage's.
    pub message: Option<Box<dyn FnOnce(Option<SaveTrust>) -> String>>,
    /// Where a failure is reported (default: the notice).
    pub fail: Option<SaveFail>,
    /// Runs with the reloaded info.
    pub after: Option<SaveAfter>,
}

/// TS: settingsFlow.tsx SettingsFlowOptions (values; `setMode`, `setBusy`, `setError`, `setStatusMessage` and
/// `onReview` are effects).
#[derive(Clone, Copy, Debug)]
pub struct SettingsFlowProps<'a> {
    pub client: Option<&'a Rc<LiveClient>>,
    pub mode: Mode,
}

/// TS: settingsFlow.tsx useSettingsFlow's state.
#[derive(Debug)]
pub struct SettingsFlow {
    pub info: Option<SettingsInfo>,
    pub cwd: String,
    pub view: SettingsView,
    pub row: usize,
    pub action_row: usize,
    /// The selected column (layer); it stays put while the row changes.
    pub column: ConfigTargetKind,
    pub edit: Option<SettingsEdit>,
    pub links: Option<LinksState>,
    pub sizes: IndexMap<String, Option<i64>>,
    pub notice: Option<Notice>,
    /// The raw JSON being edited (edit-project).
    pub document: Option<ProjectConfigDocument>,
    pub editor: EditorState,
    pub editor_error: Option<String>,
    /// Each link picker gets a token; size results for a closed or reopened picker are dropped.
    pub token: u64,
    /// The pane's per-view scroll (settingsPane.tsx's ref).
    pub scroll: SettingsScroll,
    /// The JSON editor pane's scroll (configEditorPane.tsx's ref).
    pub editor_scroll: usize,
}

impl Default for SettingsFlow {
    fn default() -> Self {
        SettingsFlow {
            info: None,
            cwd: String::new(),
            view: SettingsView::Main,
            row: 0,
            action_row: 0,
            column: ConfigTargetKind::Repository,
            edit: None,
            links: None,
            sizes: IndexMap::new(),
            notice: None,
            document: None,
            editor: EditorState::default(),
            editor_error: None,
            token: 0,
            scroll: SettingsScroll::default(),
            editor_scroll: 0,
        }
    }
}

/// The Settings keys.
pub static KEYMAP: Keymap = &[];

impl SettingsFlow {
    /// TS: SettingsFlow.open (C): load the settings of `cwd`'s repository and show them. Nothing is written.
    pub fn open(&mut self, cwd: &str, props: &SettingsFlowProps<'_>, cx: &mut FlowCx<App, Self>) {
        let _ = (cwd, props, cx);
        todo!("F4")
    }

    /// TS: useSettingsFlow load: reloads the settings of `cwd` (`fresh`: pick the initial column), then `after`.
    pub fn load(
        &mut self,
        cwd: &str,
        after: Option<LoadAfter>,
        fresh: bool,
        props: &SettingsFlowProps<'_>,
        cx: &mut FlowCx<App, Self>,
    ) {
        let _ = (cwd, after, fresh, props, cx);
        todo!("F4")
    }

    /// TS: useSettingsFlow close.
    pub fn close(&mut self, cx: &mut FlowCx<App, Self>) {
        let _ = cx;
        todo!("F4")
    }

    /// The cell the cursor is on: a global-only row always selects Global (TS `cellColumn`).
    pub fn cell_column(&self) -> ConfigTargetKind {
        todo!("F4")
    }

    /// TS: useSettingsFlow save: writes `change` into `layer` and reloads.
    pub fn save(
        &mut self,
        layer: ConfigTargetKind,
        change: SettingChange,
        label: &str,
        options: SaveOptions,
        props: &SettingsFlowProps<'_>,
        cx: &mut FlowCx<App, Self>,
    ) {
        let _ = (layer, change, label, options, props, cx);
        todo!("F4")
    }

    /// TS: useSettingsFlow saveFlag: a global-only setting stored in config.json itself.
    pub fn save_flag(
        &mut self,
        id: SettingId,
        label: &str,
        value: bool,
        props: &SettingsFlowProps<'_>,
        cx: &mut FlowCx<App, Self>,
    ) {
        let _ = (id, label, value, props, cx);
        todo!("F4")
    }

    /// TS: useSettingsFlow editJson (E): the raw JSON of `kind`'s file, reloaded first.
    pub fn edit_json(&mut self, kind: ConfigTargetKind, props: &SettingsFlowProps<'_>, cx: &mut FlowCx<App, Self>) {
        let _ = (kind, props, cx);
        todo!("F4")
    }

    /// TS: useSettingsFlow loadSizes: candidate sizes, SIZE_CHUNK paths at a time, while `token` is current.
    pub fn load_sizes(
        &mut self,
        data: &WorktreeCandidates,
        token: u64,
        props: &SettingsFlowProps<'_>,
        cx: &mut FlowCx<App, Self>,
    ) {
        let _ = (data, token, props, cx);
        todo!("F4")
    }

    /// TS: useSettingsFlow openLinks: the Linked items picker of `layer`.
    pub fn open_links(&mut self, layer: ConfigTargetKind, props: &SettingsFlowProps<'_>, cx: &mut FlowCx<App, Self>) {
        let _ = (layer, props, cx);
        todo!("F4")
    }

    /// TS: useSettingsFlow linksDirty.
    pub fn links_dirty(&self) -> bool {
        todo!("F4")
    }

    /// TS: SettingsFlow.cancelable: whether a first Ctrl+C should cancel (like Esc) instead of quitting.
    pub fn cancelable(&self, props: &SettingsFlowProps<'_>) -> bool {
        let _ = props;
        todo!("F4")
    }

    /// TS: useSettingsFlow handleEdit: keys while an edit control is open.
    pub fn handle_edit(&mut self, input: &Input, props: &SettingsFlowProps<'_>, cx: &mut FlowCx<App, Self>) {
        let _ = (input, props, cx);
        todo!("F4")
    }

    /// TS: useSettingsFlow editHelp: the edit control's help lines (live check, rules, examples, preview).
    pub fn edit_help(&self) -> Option<Vec<DetailLine>> {
        todo!("F4")
    }
}

impl Flow<App> for SettingsFlow {
    type Props<'a> = SettingsFlowProps<'a>;

    /// TS: SettingsFlow.handleInput (the caller busy-gates it).
    fn handle_input(&mut self, props: &SettingsFlowProps<'_>, input: &Input, cx: &mut FlowCx<App, Self>) {
        let _ = (props, input, cx);
        todo!("F4")
    }

    fn render(&mut self, props: &SettingsFlowProps<'_>, buf: &mut Buffer, area: Rect) {
        let _ = (props, buf, area);
        todo!("F4")
    }
}
