//! Port of settingsFlow.tsx: C → Settings. A grid of every setting with one column per layer; the cursor is a cell
//! (↑↓ setting, ←→ or Tab layer) and edits save immediately into that cell's layer through save-config's
//! revision-checked writes. E opens the raw JSON of the selected column's file and returns here; T reviews trust and
//! returns here. Saving never runs anything; a repository file stays trusted when the version it replaced was (see
//! savedProjectTrust).

use std::rc::Rc;

use indexmap::IndexMap;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use serde_json::Value;

use deckhand_client::live::LiveClient;
use deckhand_core::config_draft::format_config_json;
use deckhand_core::display::{DetailLine, HintPart, THEME, error_message};
use deckhand_core::jsstr::{byte_to_utf16, js_len, js_slice, js_to_upper, js_trim, utf16_to_byte};
use deckhand_core::key::Input;
use deckhand_core::settings_model::{
    ACTION_COMMAND_EXAMPLES, ACTION_COMMAND_HELP, ACTION_NAME_EXAMPLES, ACTION_NAME_RULES, DevEdit, SETTINGS,
    SavedMessageOptions, SettingChange, SettingControl, SettingDef, SettingId, action_name_check, action_name_problem,
    action_saved_message, apply_change, choice_change, choice_options, column_problem, command_problem, current_choice,
    derive_process_name, dev_command_change, dev_name_problem, dev_process_saved_message, dev_processes_of, entry_noun,
    info_layer, inherited_hint, initial_column, initial_links, initial_text, is_app_flag, layer_entries, layer_sets,
    link_selection, other_target, own_value, preview_location, saved_message, scroll_speed_text, setting_path,
    target_name, text_change, trust_note,
};
use deckhand_core::text_editor::{EditOptions, EditorState, edit_text};
use deckhand_core::worktree_links::{BranchVars, expand_branch_name, valid_relative_path};
use deckhand_protocol::data::{
    CandidateConfigured, CandidateKind, CandidateSuggestion, ConfigTargetKind, ConfigTargets, JsNumber,
    ProjectConfigDocument, SaveTrust, SettingsInfo, StringOrBool, WorktreeCandidate, WorktreeCandidates,
};
use deckhand_protocol::{AppConfig, Ordered};
use deckhand_sys::notify;
use deckhand_sys::storage::patch_app_config;

use crate::app::state::{App, Mode};
use crate::flow::{Flow, FlowCx, FlowEffect};
use crate::keymap::Keymap;
use crate::panes::config_editor::{ConfigEditorPaneProps, render_config_editor_pane};
use crate::panes::details::{DetailsPaneProps, render_details_pane};
use crate::panes::settings::{
    ActionStep, LinksState, Notice, ProcessStep, SettingsEdit, SettingsPaneProps, SettingsScroll, SettingsView,
    render_settings_pane,
};

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
    match id {
        SettingId::WorktreeBranchName => 200,
        SettingId::WorktreeLocation => 4096,
        _ => 8192,
    }
}

/// JS `String(number)` for the numbers a setting could hold.
fn js_number(number: &serde_json::Number) -> String {
    if number.is_i64() || number.is_u64() {
        return number.to_string();
    }
    let value = number.as_f64().unwrap_or(0.0);
    if value.fract() == 0.0 && value.abs() < 1e21 {
        format!("{value:.0}")
    } else {
        value.to_string()
    }
}

/// JS `String(value)` (`[...].join` writes `null` items as empty).
fn js_string(value: &Value, in_array: bool) -> String {
    match value {
        Value::Null if in_array => String::new(),
        Value::Null => "null".to_owned(),
        Value::Bool(flag) => flag.to_string(),
        Value::Number(number) => js_number(number),
        Value::String(text) => text.clone(),
        Value::Array(items) => items
            .iter()
            .map(|item| js_string(item, true))
            .collect::<Vec<_>>()
            .join(","),
        Value::Object(_) => "[object Object]".to_owned(),
    }
}

/// TS: settingsFlow.tsx display: a value as the status says it (`a, b` / `none`, `on`/`off`, the string).
pub fn display(value: Option<&Value>) -> String {
    match value {
        None => "undefined".to_owned(),
        Some(Value::Array(items)) => {
            let joined = items
                .iter()
                .map(|item| js_string(item, true))
                .collect::<Vec<_>>()
                .join(", ");
            if joined.is_empty() { "none".to_owned() } else { joined }
        }
        Some(Value::Bool(flag)) => (if *flag { "on" } else { "off" }).to_owned(),
        Some(other) => js_string(other, false),
    }
}

/// TS: settingsFlow.tsx capitalize (the first UTF-16 unit upper-cased; a surrogate pair's half stays as it is).
pub fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        None => String::new(),
        Some(first) if first.len_utf16() > 1 => text.to_owned(),
        Some(first) => format!("{}{}", js_to_upper(first.encode_utf8(&mut [0; 4])), chars.as_str()),
    }
}

/// Not in the TS: step 1 of adding (`name` none) or editing a Dev process of `target`: its command, starting at
/// `command`.
fn process_edit(target: ConfigTargetKind, name: Option<String>, command: String) -> SettingsEdit {
    SettingsEdit::Process {
        target,
        step: ProcessStep::Command,
        name,
        command: None,
        state: state_at_end(command),
        error: None,
    }
}

/// The command a Dev edit saves (for what a failed save says it wanted).
fn command_text(edit: &DevEdit) -> String {
    match edit {
        DevEdit::Add { command, .. } | DevEdit::Update { command, .. } => command.clone(),
        DevEdit::Remove { name } => name.clone(),
    }
}

/// The one-line input after `input` (no newlines; at most `limit` UTF-16 units, as in TS).
fn edited_line(state: &EditorState, input: &Input, limit: usize) -> EditorState {
    // One-line fields: Ctrl+A / Ctrl+E are line start / end, as in notes and tasks (select-all is the JSON editor's).
    let options = EditOptions {
        select_all: Some(false),
        ..EditOptions::default()
    };
    let next = edit_text(state, &input.input, &input.key, &options);
    // The limit and the cursor are UTF-16 units, as in TS; the cursor is not moved for the newlines dropped.
    let cursor = byte_to_utf16(&next.text, next.cursor);
    let stripped = next.text.replace('\n', "");
    let text = js_slice(&stripped, 0, Some(i64::try_from(limit).unwrap_or(i64::MAX)));
    let cursor = utf16_to_byte(&text, cursor.min(js_len(&stripped)).min(limit));
    EditorState {
        text,
        cursor,
        select_all: (next.select_all == Some(true)).then_some(true),
        message: None,
    }
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
    /// Not in the TS: what a failure says was wanted, instead of the value saved (a Dev process's command rather
    /// than the whole devCommand).
    pub wanted: Option<String>,
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

/// `info.targets[kind]`.
fn target_document(targets: &ConfigTargets, kind: ConfigTargetKind) -> Option<&ProjectConfigDocument> {
    match kind {
        ConfigTargetKind::Global => targets.global.as_ref(),
        ConfigTargetKind::Repository => targets.repository.as_ref(),
    }
}

/// Why `kind`'s document is missing: its load error, else `<Target> is unavailable`.
fn unavailable(targets: &ConfigTargets, kind: ConfigTargetKind) -> String {
    let error = match kind {
        ConfigTargetKind::Global => targets.global_error.clone(),
        ConfigTargetKind::Repository => targets.repository_error.clone(),
    };
    error.unwrap_or_else(|| format!("{} is unavailable", capitalize(target_name(kind))))
}

/// A notice (`error`: shown red).
fn notice(text: impl Into<String>, error: bool) -> Option<Notice> {
    Some(Notice {
        text: text.into(),
        error,
    })
}

/// An empty editor state with the cursor at the end of `text`.
fn state_at_end(text: String) -> EditorState {
    EditorState {
        cursor: text.len(),
        text,
        select_all: None,
        message: None,
    }
}

/// A help line.
fn line(text: impl Into<String>) -> DetailLine {
    DetailLine {
        text: text.into(),
        ..DetailLine::default()
    }
}

/// A colored help line.
fn colored(text: impl Into<String>, color: &'static str) -> DetailLine {
    DetailLine {
        color: Some(color),
        ..line(text)
    }
}

/// A help line cut instead of wrapped.
fn nowrap(text: impl Into<String>) -> DetailLine {
    DetailLine {
        nowrap: Some(true),
        ..line(text)
    }
}

/// The list cursor keys (TS `move`): ↑/k, ↓/j, PageUp/PageDown by 5, Home/End.
fn move_cursor(input: &Input, current: usize, last: usize) -> Option<usize> {
    let key = &input.key;
    if key.up_arrow || input.input == "k" {
        return Some(current.saturating_sub(1));
    }
    if key.down_arrow || input.input == "j" {
        return Some(last.min(current + 1));
    }
    if key.page_up || key.page_down {
        let next = if key.page_up {
            current.saturating_sub(5)
        } else {
            current.saturating_add(5)
        };
        return Some(last.min(next));
    }
    if key.home || key.end {
        return Some(if key.home { 0 } else { last });
    }
    None
}

impl SettingsFlow {
    /// TS: SettingsFlow.open (C): load the settings of `cwd`'s repository and show them. Nothing is written.
    pub fn open(&mut self, cwd: &str, props: &SettingsFlowProps<'_>, cx: &mut FlowCx<App, Self>) {
        cx.set_error(None);
        cx.set_status(None);
        self.notice = None;
        self.view = SettingsView::Main;
        self.row = 0;
        self.action_row = 0;
        self.load(cwd, None, true, props, cx);
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
        self.load_with(props.client, cwd, after, fresh, cx);
    }

    /// `load` with the client the caller had (continuations keep the one of the call that started them).
    fn load_with(
        &mut self,
        client: Option<&Rc<LiveClient>>,
        cwd: &str,
        after: Option<LoadAfter>,
        fresh: bool,
        cx: &mut FlowCx<App, Self>,
    ) {
        let Some(client) = client else { return };
        cx.set_busy(true);
        // The column of the call (TS: the closure's), for the action row's bound.
        let column = self.column;
        let list_id = self.list_id();
        let target_cwd = cwd.to_owned();
        // Each result releases busy in the same render that shows it, so keys typed right after it are not dropped.
        cx.spawn(client.settings_info(cwd), move |flow: &mut SettingsFlow, result, cx| {
            match result {
                Ok(next) => {
                    cx.set_busy(false);
                    flow.token += 1;
                    flow.info = Some(next.clone());
                    flow.cwd = target_cwd;
                    flow.edit = None;
                    flow.links = None;
                    if flow.view == SettingsView::Links {
                        flow.view = SettingsView::Main;
                    }
                    if fresh || column_problem(&next.targets, flow.column).is_some() {
                        flow.column = initial_column(&next.targets);
                    }
                    flow.action_row = flow.action_row.min(layer_entries(&next, list_id, column).own.len());
                    cx.emit(FlowEffect::SetMode(Mode::Settings));
                    if let Some(after) = after {
                        after(flow, &next, cx);
                    }
                }
                Err(error) => cx.set_error(Some(error_message(&error))),
            }
            cx.set_busy(false);
        });
    }

    /// TS: useSettingsFlow close.
    pub fn close(&mut self, cx: &mut FlowCx<App, Self>) {
        self.token += 1;
        self.edit = None;
        self.links = None;
        cx.emit(FlowEffect::SetMode(Mode::Browse));
    }

    /// The setting under the cursor (TS `def`).
    fn def(&self) -> SettingDef {
        SETTINGS[self.row.min(SETTINGS.len() - 1)]
    }

    /// The named-command list the list view edits: Actions, or (not in the TS) the Dev command's processes.
    pub fn list_id(&self) -> SettingId {
        match self.def().id {
            SettingId::DevCommand => SettingId::DevCommand,
            _ => SettingId::Actions,
        }
    }

    /// Not in the TS: Enter on the Dev command lists what Dev runs: its one command, or its processes, then
    /// + Add a process.
    fn open_dev_processes(&mut self) {
        self.view = SettingsView::Actions;
        self.action_row = 0;
    }

    /// The cell the cursor is on: a global-only row always selects Global (TS `cellColumn`).
    pub fn cell_column(&self) -> ConfigTargetKind {
        // A global-only row has nothing to edit in This repo, while `column` keeps the picked column for the rows
        // around it.
        if self.def().global_only {
            ConfigTargetKind::Global
        } else {
            self.column
        }
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
        let Some(client) = props.client else { return };
        let Some(info) = self.info.as_ref() else { return };
        let SaveOptions {
            cleared,
            extra,
            message,
            fail,
            after,
            wanted,
        } = options;
        let report = |flow: &mut SettingsFlow, text: String| match fail {
            Some(fail) => fail(flow, text),
            None => flow.notice = notice(text, true),
        };
        let Some(document) = target_document(&info.targets, layer) else {
            let text = unavailable(&info.targets, layer);
            report(self, text);
            return;
        };
        let raw = match apply_change(document, &change) {
            Ok(raw) => raw,
            Err(error) => {
                let text = format!("{label} was not saved: {}", error_message(&error));
                report(self, text);
                return;
            }
        };
        cx.set_busy(true);
        let request = client.save_config(layer, &self.cwd, &raw, document.revision.as_deref());
        let client = Rc::clone(client);
        let cwd = self.cwd.clone();
        let label = label.to_owned();
        cx.spawn(request, move |flow: &mut SettingsFlow, result, cx| match result {
            Ok(saved) => {
                cx.set_busy(false);
                flow.notice = None;
                let trust = saved.trust;
                // The status appears with the reloaded screen, once it accepts keys again.
                let after: LoadAfter = Box::new(move |flow, next, cx| {
                    let text = match message {
                        Some(message) => message(trust),
                        None => saved_message(&label, layer, SavedMessageOptions { cleared, trust }),
                    };
                    cx.set_status(Some(format!("{text}{extra}")));
                    if let Some(after) = after {
                        after(flow, next);
                    }
                });
                flow.load_with(Some(&client), &cwd, Some(after), false, cx);
            }
            Err(error) => {
                cx.set_busy(false);
                let message = error_message(&error);
                let attempt = if cleared {
                    "clearing it".to_owned()
                } else {
                    format!("wanted {}", wanted.unwrap_or_else(|| display(change.value.as_ref())))
                };
                let text = if message.contains("changed on disk") {
                    format!(
                        "{} changed on disk, so {label} was not saved ({attempt}). Reloaded; try again.",
                        capitalize(target_name(layer))
                    )
                } else {
                    format!("{label} was not saved ({attempt}): {message}")
                };
                let after: LoadAfter = Box::new(move |flow, _next, _cx| flow.notice = notice(text, true));
                flow.load_with(Some(&client), &cwd, Some(after), false, cx);
            }
        });
    }

    /// Not in the TS: saves the Scroll speed (`scroll_speed` in config.json), which Preview focus takes at once and
    /// attaching reads when it starts.
    pub fn save_scroll_speed(
        &mut self,
        label: &str,
        speed: f64,
        props: &SettingsFlowProps<'_>,
        cx: &mut FlowCx<App, Self>,
    ) {
        cx.set_busy(true);
        let patch = AppConfig {
            scroll_speed: Some(JsNumber(speed)),
            ..AppConfig::default()
        };
        let client = props.client.cloned();
        let cwd = self.cwd.clone();
        let label = label.to_owned();
        cx.spawn(
            patch_app_config(patch),
            move |flow: &mut SettingsFlow, result, cx| match result {
                Ok(_) => {
                    cx.set_busy(false);
                    flow.notice = None;
                    cx.emit(FlowEffect::ScrollSpeed(speed));
                    let after: LoadAfter = Box::new(move |_flow, _next, cx| {
                        cx.set_status(Some(format!(
                            "{label} {} (global) · applies when attached and in Preview focus",
                            scroll_speed_text(speed)
                        )));
                    });
                    flow.load_with(client.as_ref(), &cwd, Some(after), false, cx);
                }
                Err(error) => {
                    cx.set_busy(false);
                    flow.notice = notice(format!("{label} was not saved: {}", error_message(&error)), true);
                }
            },
        );
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
        cx.set_busy(true);
        let patch = match id {
            SettingId::AgentHooks => AppConfig {
                agent_hooks: Some(value),
                ..AppConfig::default()
            },
            _ => AppConfig {
                notifications: Some(value),
                ..AppConfig::default()
            },
        };
        let client = props.client.cloned();
        let cwd = self.cwd.clone();
        let label = label.to_owned();
        cx.spawn(
            patch_app_config(patch),
            move |flow: &mut SettingsFlow, result, cx| match result {
                Ok(_) => {
                    cx.set_busy(false);
                    flow.notice = None;
                    let test_notification = id == SettingId::Notifications && value;
                    // The Codex hint (if any) shows under the row, which stays selected.
                    let after: LoadAfter = Box::new(move |_flow, _next, cx| {
                        let state = if value { "on" } else { "off" };
                        let applies = if test_notification {
                            " · sent a test notification: if macOS asks, allow Deckhand"
                        } else if value {
                            " · applies to new and restarted sessions"
                        } else {
                            ""
                        };
                        cx.set_status(Some(format!("{label} {state} (global){applies}")));
                    });
                    flow.load_with(client.as_ref(), &cwd, Some(after), false, cx);
                    if test_notification {
                        // Not in the TS: one notification now, so macOS's permission prompt comes up while you are
                        // looking rather than when a session first needs you. Settings reloads once it is answered,
                        // and its row then says if macOS blocks Deckhand.
                        cx.spawn(
                            notify::send(
                                "Deckhand",
                                "Notifications are on. You'll get one when a session needs you or exits.",
                            ),
                            move |flow: &mut SettingsFlow, _delivery, cx| {
                                flow.load_with(client.as_ref(), &cwd, None, false, cx);
                            },
                        );
                    }
                }
                Err(error) => {
                    cx.set_busy(false);
                    flow.notice = notice(format!("{label} was not saved: {}", error_message(&error)), true);
                }
            },
        );
    }

    /// TS: useSettingsFlow editJson (E): the raw JSON of `kind`'s file, reloaded first.
    pub fn edit_json(&mut self, kind: ConfigTargetKind, props: &SettingsFlowProps<'_>, cx: &mut FlowCx<App, Self>) {
        let Some(client) = props.client else { return };
        cx.set_busy(true);
        // Reload so the editor opens the current bytes and revision.
        cx.spawn(
            client.settings_info(&self.cwd),
            move |flow: &mut SettingsFlow, result, cx| {
                match result {
                    Ok(next) => {
                        cx.set_busy(false);
                        let opened = target_document(&next.targets, kind).cloned();
                        let missing = opened.is_none().then(|| unavailable(&next.targets, kind));
                        flow.info = Some(next);
                        match opened {
                            None => flow.notice = notice(missing.unwrap_or_default(), true),
                            Some(opened) => {
                                flow.token += 1;
                                flow.edit = None;
                                flow.links = None;
                                if flow.view == SettingsView::Links {
                                    flow.view = SettingsView::Main;
                                }
                                flow.editor = EditorState {
                                    text: opened.raw.clone(),
                                    cursor: 0,
                                    select_all: None,
                                    message: None,
                                };
                                flow.document = Some(opened);
                                flow.editor_error = None;
                                cx.emit(FlowEffect::SetMode(Mode::EditProject));
                            }
                        }
                    }
                    Err(error) => cx.set_error(Some(error_message(&error))),
                }
                cx.set_busy(false);
            },
        );
    }

    /// TS: useSettingsFlow loadSizes: candidate sizes, SIZE_CHUNK paths at a time, while `token` is current.
    pub fn load_sizes(
        &mut self,
        data: &WorktreeCandidates,
        token: u64,
        props: &SettingsFlowProps<'_>,
        cx: &mut FlowCx<App, Self>,
    ) {
        let Some(client) = props.client else { return };
        let paths: Vec<String> = data
            .candidates
            .iter()
            .filter(|candidate| candidate.missing != Some(true) && candidate.kind != CandidateKind::Symlink)
            .map(|candidate| candidate.path.clone())
            .collect();
        let cwd = self.cwd.clone();
        self.load_size_chunk(Rc::clone(client), cwd, Rc::new(paths), 0, token, cx);
    }

    /// One turn of loadSizes' loop: the chunk at `index`, then the next while the picker's token is current.
    fn load_size_chunk(
        &mut self,
        client: Rc<LiveClient>,
        cwd: String,
        paths: Rc<Vec<String>>,
        index: usize,
        token: u64,
        cx: &mut FlowCx<App, Self>,
    ) {
        if index >= paths.len() || self.token != token {
            return;
        }
        let chunk: Vec<String> = paths.iter().skip(index).take(SIZE_CHUNK).cloned().collect();
        let request = client.worktree_candidate_sizes(&cwd, &chunk);
        cx.spawn(request, move |flow: &mut SettingsFlow, result, cx| {
            let result = result.unwrap_or_else(|_| chunk.into_iter().map(|file| (file, None)).collect());
            if flow.token != token {
                return;
            }
            flow.sizes.extend(result);
            flow.load_size_chunk(client, cwd, paths, index + SIZE_CHUNK, token, cx);
        });
    }

    /// TS: useSettingsFlow openLinks: the Linked items picker of `layer`.
    pub fn open_links(&mut self, layer: ConfigTargetKind, props: &SettingsFlowProps<'_>, cx: &mut FlowCx<App, Self>) {
        let Some(client) = props.client else { return };
        self.token += 1;
        let current = self.token;
        cx.set_busy(true);
        // Global links apply to every repository: its picker lists what your repositories share, nothing preselected
        // (sizes are one checkout's, so it has none).
        let global = layer == ConfigTargetKind::Global;
        let request = if global {
            client.global_link_candidates(&self.cwd)
        } else {
            client.worktree_candidates(&self.cwd)
        };
        let client = Rc::clone(client);
        cx.spawn(request, move |flow: &mut SettingsFlow, result, cx| {
            match result {
                Ok(data) => {
                    cx.set_busy(false);
                    if flow.token == current {
                        let candidates: Vec<WorktreeCandidate> =
                            data.candidates.iter().map(|candidate| (**candidate).clone()).collect();
                        let selected = initial_links(&candidates, !global);
                        flow.links = Some(LinksState {
                            data: data.clone(),
                            links: selected.clone(),
                            initial: selected,
                            target: layer,
                            row: 0,
                        });
                        flow.sizes = IndexMap::new();
                        flow.view = SettingsView::Links;
                        flow.notice = None;
                        if !global {
                            let props = SettingsFlowProps {
                                client: Some(&client),
                                mode: Mode::Settings,
                            };
                            flow.load_sizes(&data, current, &props, cx);
                        }
                    }
                }
                Err(error) => flow.notice = notice(error_message(&error), true),
            }
            cx.set_busy(false);
        });
    }

    /// TS: useSettingsFlow linksDirty.
    pub fn links_dirty(&self) -> bool {
        // JSON.stringify compares in key order too.
        self.links
            .as_ref()
            .is_some_and(|links| links.links.iter().ne(links.initial.iter()))
    }

    /// TS: SettingsFlow.cancelable: whether a first Ctrl+C should cancel (like Esc) instead of quitting.
    pub fn cancelable(&self, props: &SettingsFlowProps<'_>) -> bool {
        props.mode == Mode::EditProject
            || (props.mode == Mode::Settings
                && (self.edit.is_some() || (self.view == SettingsView::Links && self.links_dirty())))
    }

    /// T: the inline trust review of the repository; Settings reload when it closes.
    fn review(&self, props: &SettingsFlowProps<'_>, cx: &mut FlowCx<App, Self>) {
        let client = props.client.cloned();
        let cwd = self.cwd.clone();
        let back =
            cx.callback(move |flow: &mut SettingsFlow, cx| flow.load_with(client.as_ref(), &cwd, None, false, cx));
        cx.emit(FlowEffect::Review {
            cwd: self.cwd.clone(),
            back,
        });
    }

    /// TS: useSettingsFlow handleEdit: keys while an edit control is open.
    pub fn handle_edit(&mut self, input: &Input, props: &SettingsFlowProps<'_>, cx: &mut FlowCx<App, Self>) {
        let Some(current) = self.edit.clone() else { return };
        if self.info.is_none() {
            return;
        }
        let key = &input.key;
        if current == SettingsEdit::DiscardLinks {
            if key.escape {
                self.edit = None;
            } else if key.return_ {
                self.token += 1;
                self.edit = None;
                self.links = None;
                self.view = SettingsView::Main;
            }
            return;
        }
        if key.escape {
            self.edit = None;
            return;
        }
        match current {
            SettingsEdit::DiscardLinks => {}
            SettingsEdit::Clear {
                id,
                label,
                target,
                entry,
            } => {
                if key.return_ {
                    let process = entry.as_deref().filter(|_| id == SettingId::DevCommand);
                    let change = match (process, self.info.as_ref()) {
                        (Some(name), Some(info)) => {
                            match dev_command_change(&info.targets, target, &DevEdit::Remove { name: name.to_owned() })
                            {
                                Ok(change) => change,
                                Err(error) => {
                                    self.notice = notice(error, true);
                                    return;
                                }
                            }
                        }
                        _ => SettingChange {
                            path: setting_path(id, entry.as_deref()),
                            value: None,
                        },
                    };
                    let options = SaveOptions {
                        cleared: true,
                        ..SaveOptions::default()
                    };
                    self.save(target, change, &label, options, props, cx);
                }
            }
            SettingsEdit::Choice {
                id,
                label,
                target,
                options,
                index,
                current: stored,
            } => self.handle_choice(id, &label, target, &options, index, stored, input, props, cx),
            SettingsEdit::Text { .. } => self.handle_text(current, input, props, cx),
            SettingsEdit::Process { .. } => self.handle_process(current, input, props, cx),
        }
    }

    /// handleEdit for a choice control.
    #[allow(clippy::too_many_arguments)]
    fn handle_choice(
        &mut self,
        id: SettingId,
        label: &str,
        target: ConfigTargetKind,
        options: &[deckhand_core::settings_model::ChoiceOption],
        index: usize,
        stored: Option<usize>,
        input: &Input,
        props: &SettingsFlowProps<'_>,
        cx: &mut FlowCx<App, Self>,
    ) {
        let key = &input.key;
        let count = options.len();
        if count == 0 {
            return;
        }
        let choice = |index: usize| SettingsEdit::Choice {
            id,
            label: label.to_owned(),
            target,
            options: options.to_vec(),
            index,
            current: stored,
        };
        if key.up_arrow || input.input == "k" {
            self.edit = Some(choice((index % count + count - 1) % count));
        } else if key.down_arrow || input.input == "j" {
            self.edit = Some(choice((index + 1) % count));
        } else if key.return_ {
            let Some(option) = options.get(index) else { return };
            if option.custom == Some(true) {
                let text = match &option.value {
                    Some(StringOrBool::String(text)) => text.clone(),
                    _ => String::new(),
                };
                self.edit = Some(SettingsEdit::Text {
                    id,
                    label: label.to_owned(),
                    target,
                    state: state_at_end(text),
                    error: None,
                    entry: None,
                    step: None,
                    placeholder: Some("a template with {name}, e.g. ~/worktrees/{repo}/{name}".to_owned()),
                });
                return;
            }
            let Some(info) = self.info.as_ref() else { return };
            let Some(change) = choice_change(info, id, target, option) else {
                self.edit = None;
                let why = if is_app_flag(id) {
                    "it is already".to_owned()
                } else {
                    format!("{} already has it", target_name(target))
                };
                self.notice = notice(format!("{label} unchanged: {why}"), false);
                return;
            };
            if id == SettingId::ScrollSpeed {
                if let Some(StringOrBool::String(text)) = &option.value
                    && let Ok(speed) = text.parse::<f64>()
                {
                    self.save_scroll_speed(label, speed, props, cx);
                }
                return;
            }
            if is_app_flag(id) {
                let on = option.value == Some(StringOrBool::Bool(true));
                self.save_flag(id, label, on, props, cx);
                return;
            }
            let options = SaveOptions {
                cleared: change.value.is_none(),
                ..SaveOptions::default()
            };
            self.save(target, change, label, options, props, cx);
        }
    }

    /// handleEdit for a text control (a setting, an action's name or command, a path to link).
    fn handle_text(
        &mut self,
        current: SettingsEdit,
        input: &Input,
        props: &SettingsFlowProps<'_>,
        cx: &mut FlowCx<App, Self>,
    ) {
        let SettingsEdit::Text {
            id,
            label,
            target,
            state,
            entry,
            step,
            ..
        } = &current
        else {
            return;
        };
        let Some(info) = self.info.as_ref() else { return };
        let (id, target) = (*id, *target);
        let with_error = |error: Option<String>| {
            let mut edit = current.clone();
            if let SettingsEdit::Text { error: slot, .. } = &mut edit {
                *slot = error;
            }
            edit
        };
        let key = &input.key;
        if key.return_ {
            let text = state.text.clone();
            if *step == Some(ActionStep::Name) {
                // An invalid name keeps the step open; the live check already shows why (an empty name says so now).
                let problem = action_name_check(&info.targets, target, &text)
                    .error
                    .or_else(|| action_name_problem(&text));
                if let Some(problem) = problem {
                    if text.is_empty() {
                        self.edit = Some(with_error(Some(problem)));
                    }
                    return;
                }
                let command = initial_text(&info.targets, id, target, Some(&text));
                let placeholder =
                    inherited_hint(info, id, target, Some(&text)).filter(|placeholder| !placeholder.is_empty());
                self.edit = Some(SettingsEdit::Text {
                    id,
                    label: format!("{} {text}", capitalize(entry_noun(id))),
                    target,
                    state: state_at_end(command),
                    error: None,
                    entry: Some(text),
                    step: Some(ActionStep::Command),
                    placeholder,
                });
                return;
            }
            // a in Linked items: a path typed in, listed (linked) until Enter saves the picker.
            if id == SettingId::WorktreeLinks
                && let Some(links) = self.links.as_ref()
            {
                let entry = js_trim(&text).trim_end_matches('/').to_owned();
                if !valid_relative_path(&entry) {
                    self.edit = Some(with_error(Some(
                        "A path relative to the repository root (no leading /, no ..)".to_owned(),
                    )));
                    return;
                }
                self.edit = None;
                let mut next = links.clone();
                next.links.insert(entry.clone(), true);
                if let Some(at) = links
                    .data
                    .candidates
                    .iter()
                    .position(|candidate| candidate.path == entry)
                {
                    next.row = at;
                    self.links = Some(next);
                    self.notice = notice(
                        format!("{entry} is already listed; it is linked now (Enter saves)"),
                        false,
                    );
                    return;
                }
                // After the linked rows, so it shows with them.
                let end = links
                    .data
                    .candidates
                    .iter()
                    .filter(|candidate| candidate.configured.is_some() || candidate.added == Some(true))
                    .count();
                let added = WorktreeCandidate {
                    path: entry,
                    kind: CandidateKind::File,
                    target: None,
                    ignored: false,
                    suggestion: CandidateSuggestion::Skip,
                    reason: String::new(),
                    configured: None,
                    source: None,
                    layers: None,
                    repos: None,
                    missing: None,
                    added: Some(true),
                };
                next.data.candidates.insert(end, Ordered::new(added));
                next.row = end;
                self.links = Some(next);
                self.notice = None;
                return;
            }
            let change = match text_change(id, &text, entry.as_deref()) {
                Ok(change) => change,
                Err(error) => {
                    self.edit = Some(with_error(Some(error)));
                    return;
                }
            };
            // Enter on the layer's own value unchanged saves nothing.
            let layer = info_layer(&info.targets, target);
            if own_value(layer.as_ref(), id, entry.as_deref()).and_then(Value::as_str) == Some(text.as_str()) {
                self.edit = None;
                self.notice = notice(format!("{label} unchanged"), false);
                return;
            }
            let failed = current.clone();
            let fail: SaveFail = Box::new(move |flow, message| {
                let mut edit = failed;
                if let SettingsEdit::Text { error, .. } = &mut edit {
                    *error = Some(message);
                }
                flow.edit = Some(edit);
            });
            let mut options = SaveOptions {
                fail: Some(fail),
                ..SaveOptions::default()
            };
            if let Some(name) = entry.clone() {
                let shown = name.clone();
                options.message = Some(Box::new(move |trust| action_saved_message(&shown, target, trust)));
                options.after = Some(Box::new(move |flow, next| {
                    flow.action_row = layer_entries(next, id, target)
                        .own
                        .iter()
                        .position(|action| action.name == name)
                        .unwrap_or(0);
                }));
            }
            let label = label.clone();
            self.save(target, change, &label, options, props, cx);
            return;
        }
        if key.tab || key.up_arrow || key.down_arrow {
            return;
        }
        let limit = if *step == Some(ActionStep::Name) {
            48
        } else {
            max_text(id)
        };
        let mut edit = current.clone();
        if let SettingsEdit::Text { state, error, .. } = &mut edit {
            *state = edited_line(state, input, limit);
            *error = None;
        }
        self.edit = Some(edit);
    }

    /// Not in the TS: handleEdit for a Dev process, added or edited in two steps: its command, then its name
    /// (suggested from the command for a new one, its own when editing). Each save writes the whole devCommand.
    fn handle_process(
        &mut self,
        current: SettingsEdit,
        input: &Input,
        props: &SettingsFlowProps<'_>,
        cx: &mut FlowCx<App, Self>,
    ) {
        let SettingsEdit::Process {
            target,
            step,
            name,
            command,
            state,
            ..
        } = &current
        else {
            return;
        };
        let Some(info) = self.info.as_ref() else { return };
        let (target, step) = (*target, *step);
        let with_error = |error: String| {
            let mut edit = current.clone();
            if let SettingsEdit::Process { error: slot, .. } = &mut edit {
                *slot = Some(error);
            }
            edit
        };
        let key = &input.key;
        if key.return_ {
            let text = state.text.clone();
            let processes = dev_processes_of(&info.targets, target);
            if step == ProcessStep::Command {
                if let Some(problem) = command_problem(&text) {
                    let problem = if js_trim(&text).is_empty() {
                        String::from("Type a command")
                    } else {
                        problem
                    };
                    self.edit = Some(with_error(problem));
                    return;
                }
                // Step 2: the name, its own when editing, else suggested from the command.
                let suggested = name.clone().unwrap_or_else(|| {
                    let taken: Vec<&str> = processes.iter().map(|(name, _)| name.as_str()).collect();
                    derive_process_name(&text, &taken)
                });
                self.edit = Some(SettingsEdit::Process {
                    target,
                    step: ProcessStep::Name,
                    name: name.clone(),
                    command: Some(text),
                    state: state_at_end(suggested),
                    error: None,
                });
                return;
            }
            let command = command.clone().unwrap_or_default();
            let edit = match name {
                Some(name) => {
                    let unchanged = processes
                        .iter()
                        .any(|(entry, own)| entry == name && *entry == text && *own == command);
                    if unchanged {
                        self.edit = None;
                        self.notice = notice(format!("Dev process {name} unchanged"), false);
                        return;
                    }
                    DevEdit::Update {
                        name: name.clone(),
                        new_name: text.clone(),
                        command,
                    }
                }
                None => DevEdit::Add {
                    name: text.clone(),
                    command,
                },
            };
            let change = match dev_command_change(&info.targets, target, &edit) {
                Ok(change) => change,
                Err(error) => {
                    self.edit = Some(with_error(error));
                    return;
                }
            };
            let failed = current.clone();
            let fail: SaveFail = Box::new(move |flow, message| {
                let mut edit = failed;
                if let SettingsEdit::Process { error, .. } = &mut edit {
                    *error = Some(message);
                }
                flow.edit = Some(edit);
            });
            let shown = text.clone();
            let options = SaveOptions {
                wanted: Some(command_text(&edit)),
                fail: Some(fail),
                message: Some(Box::new(move |trust| dev_process_saved_message(&shown, target, trust))),
                after: Some(Box::new(move |flow, next| {
                    let processes = dev_processes_of(&next.targets, target);
                    flow.action_row = processes.iter().position(|(name, _)| *name == text).unwrap_or(0);
                })),
                ..SaveOptions::default()
            };
            let label = format!("Dev process {}", state.text);
            self.save(target, change, &label, options, props, cx);
            return;
        }
        if key.tab || key.up_arrow || key.down_arrow {
            return;
        }
        let limit = if step == ProcessStep::Command {
            max_text(SettingId::DevCommand)
        } else {
            48
        };
        let mut edit = current.clone();
        if let SettingsEdit::Process { state, error, .. } = &mut edit {
            *state = edited_line(state, input, limit);
            *error = None;
        }
        self.edit = Some(edit);
    }

    /// Not in the TS: the help lines of a Dev process edit (what a command runs as; the live check of a name).
    fn process_help(&self) -> Option<Vec<DetailLine>> {
        let Some(SettingsEdit::Process {
            target,
            step,
            name,
            state,
            error,
            ..
        }) = self.edit.as_ref()
        else {
            return None;
        };
        let info = self.info.as_ref()?;
        let text = state.text.as_str();
        if *step == ProcessStep::Command {
            let problem = if js_trim(text).is_empty() {
                None
            } else {
                command_problem(text)
            };
            return Some(
                problem
                    .map(|problem| colored(problem, THEME.error))
                    .into_iter()
                    .chain([
                        line(
                            "Runs in the Dev tab with your shell, in the session's worktree · && cd env vars all work",
                        ),
                        line("e.g. npm run dev · cd backend && .venv/bin/python main.py · cd frontend && npm run dev"),
                    ])
                    .collect(),
            );
        }
        let mut lines = Vec::new();
        if !text.is_empty()
            && let Some(problem) = dev_name_problem(&info.targets, *target, text, name.as_deref())
            && Some(&problem) != error.as_ref()
        {
            lines.push(colored(problem, THEME.error));
        }
        lines.push(line(
            "Its label in the Dev tab and the run list (r) · Enter keeps the suggestion",
        ));
        lines.push(line(ACTION_NAME_RULES));
        Some(lines)
    }

    /// TS: useSettingsFlow editHelp: the edit control's help lines (live check, rules, examples, preview).
    pub fn edit_help(&self) -> Option<Vec<DetailLine>> {
        if matches!(self.edit, Some(SettingsEdit::Process { .. })) {
            return self.process_help();
        }
        let Some(SettingsEdit::Text {
            id,
            target,
            state,
            error,
            step,
            ..
        }) = self.edit.as_ref()
        else {
            return None;
        };
        let info = self.info.as_ref()?;
        let text = state.text.as_str();
        let blank = js_trim(text).is_empty();
        if *step == Some(ActionStep::Name) {
            let check = action_name_check(&info.targets, *target, text);
            let mut lines = Vec::new();
            match (&check.error, &check.note) {
                (Some(problem), _) if Some(problem) != error.as_ref() => lines.push(colored(problem, THEME.error)),
                (_, Some(note)) => lines.push(colored(note, THEME.active)),
                _ => {}
            }
            lines.push(line(ACTION_NAME_RULES));
            lines.push(line(ACTION_NAME_EXAMPLES));
            return Some(lines);
        }
        if *id == SettingId::WorktreeLinks && self.view == SettingsView::Links {
            return Some(vec![line(if *target == ConfigTargetKind::Global {
                "Relative to each repository's root: every new worktree links its repository's own copy; a repository without it skips it."
            } else {
                "Relative to the repository root: each new worktree links the main checkout's copy."
            })]);
        }
        Some(match id {
            SettingId::Actions => {
                let problem = if blank { None } else { command_problem(text) };
                problem
                    .map(|problem| colored(problem, THEME.error))
                    .into_iter()
                    .chain([line(ACTION_COMMAND_HELP), line(ACTION_COMMAND_EXAMPLES)])
                    .collect()
            }
            SettingId::WorktreeBranchName if blank => vec![line("A template with {name}; {user} is your user name")],
            SettingId::WorktreeBranchName => {
                let vars = BranchVars {
                    name: "my-task",
                    user: &info.user,
                };
                match expand_branch_name(text, &vars) {
                    Ok(branch) => vec![nowrap(format!("→ {branch} for a session named my-task"))],
                    Err(error) => vec![line(error_message(&error))],
                }
            }
            SettingId::WorktreeLocation if blank => {
                vec![line(
                    "A template with {name}: {repo} {repoParent} {repoRoot} {home}, ~/",
                )]
            }
            SettingId::WorktreeLocation => vec![nowrap(format!("→ {}", preview_location(text, info.vars.as_ref())))],
            SettingId::DevCommand => {
                vec![line(
                    "Runs in the Dev pane (d d starts it), with your shell in the session's worktree",
                )]
            }
            _ => vec![line("Runs in a new worktree before the agent starts")],
        })
    }

    /// handleInput in edit-project: the raw JSON editor.
    fn handle_editor(&mut self, props: &SettingsFlowProps<'_>, input: &Input, cx: &mut FlowCx<App, Self>) {
        let key = &input.key;
        let cwd = self.cwd.clone();
        let Some(document) = self.document.as_ref() else {
            self.load(&cwd, None, false, props, cx);
            return;
        };
        if key.escape {
            if self.editor.text != document.raw {
                cx.emit(FlowEffect::SetMode(Mode::DiscardProject));
            } else {
                self.load(&cwd, None, false, props, cx);
            }
            return;
        }
        if key.ctrl
            && input.input == "s"
            && let Some(client) = props.client
        {
            cx.set_busy(true);
            self.editor_error = None;
            let request = client.save_config(document.kind, &cwd, &self.editor.text, document.revision.as_deref());
            let client = Rc::clone(client);
            cx.spawn(request, move |flow: &mut SettingsFlow, result, cx| match result {
                Ok(saved) => {
                    let status = if saved.document.kind == ConfigTargetKind::Global {
                        "Saved global defaults.".to_owned()
                    } else {
                        format!("Saved deckhand.json · {}. Nothing ran.", trust_note(saved.trust))
                    };
                    flow.document = Some(saved.document);
                    cx.set_busy(false);
                    let after: LoadAfter = Box::new(move |_flow, _next, cx| cx.set_status(Some(status)));
                    flow.load_with(Some(&client), &cwd, Some(after), false, cx);
                }
                Err(error) => {
                    cx.set_busy(false);
                    flow.editor_error = Some(error_message(&error));
                }
            });
            return;
        }
        if key.ctrl && input.input == "f" {
            match format_config_json(&self.editor.text) {
                Ok(text) => {
                    // The cursor keeps its UTF-16 index (clamped to the formatted text).
                    let index = byte_to_utf16(&self.editor.text, self.editor.cursor).min(js_len(&text));
                    let cursor = utf16_to_byte(&text, index);
                    self.editor = EditorState {
                        text,
                        cursor,
                        select_all: None,
                        message: None,
                    };
                    self.editor_error = None;
                }
                Err(error) => self.editor_error = Some(error_message(&error)),
            }
            return;
        }
        self.editor_error = None;
        self.editor = edit_text(&self.editor, &input.input, key, &EditOptions::default());
    }

    /// handleInput in the link picker.
    fn handle_links(&mut self, props: &SettingsFlowProps<'_>, input: &Input, cx: &mut FlowCx<App, Self>) {
        let (Some(info), Some(links)) = (self.info.as_ref(), self.links.as_ref()) else {
            return;
        };
        let key = &input.key;
        if let Some(moved) = move_cursor(input, links.row, links.data.candidates.len().saturating_sub(1)) {
            if let Some(links) = self.links.as_mut() {
                links.row = moved;
            }
            self.notice = None;
            return;
        }
        if key.escape {
            if self.links_dirty() {
                self.edit = Some(SettingsEdit::DiscardLinks);
            } else {
                self.token += 1;
                self.links = None;
                self.view = SettingsView::Main;
            }
            return;
        }
        let candidate = links.data.candidates.get(links.row);
        if input.input == " "
            && let Some(candidate) = candidate
        {
            if candidate.configured == Some(CandidateConfigured::Files) {
                let source = candidate.source.as_deref().unwrap_or("undefined");
                self.notice = notice(
                    format!(
                        "{} comes from worktree.files (→ {source}); E edits it in the raw JSON.",
                        candidate.path
                    ),
                    false,
                );
            } else {
                let path = candidate.path.clone();
                if let Some(links) = self.links.as_mut() {
                    let linked = links.links.get(&path).copied().unwrap_or(false);
                    links.links.insert(path, !linked);
                }
                self.notice = None;
            }
            return;
        }
        if input.input == "a" {
            self.edit = Some(SettingsEdit::Text {
                id: SettingId::WorktreeLinks,
                label: "Add a path".to_owned(),
                target: links.target,
                state: EditorState::default(),
                error: None,
                entry: None,
                step: None,
                placeholder: Some(".env, .claude/settings.local.json".to_owned()),
            });
            return;
        }
        if input.input == "E" {
            if self.links_dirty() {
                self.notice = notice(
                    "Unsaved link changes: Enter saves them (Esc discards) before E opens the raw JSON.",
                    true,
                );
                return;
            }
            let target = links.target;
            self.edit_json(target, props, cx);
            return;
        }
        if key.return_ {
            if links
                .data
                .candidates_error
                .as_deref()
                .is_some_and(|error| !error.is_empty())
            {
                self.notice = notice("Nothing to save: the candidates could not be listed.", true);
                return;
            }
            let other = if links.target == ConfigTargetKind::Global {
                ConfigTargetKind::Repository
            } else {
                ConfigTargetKind::Global
            };
            let target_layer = info_layer(&info.targets, links.target);
            let before = own_value(target_layer.as_ref(), SettingId::WorktreeLinks, None);
            // Global covers every repository, so this repo's list never makes a global entry redundant (the reverse
            // does).
            let other_layer = if links.target == ConfigTargetKind::Global {
                None
            } else {
                info_layer(&info.targets, other)
            };
            let candidates: Vec<WorktreeCandidate> = links
                .data
                .candidates
                .iter()
                .map(|candidate| (**candidate).clone())
                .collect();
            let selection = link_selection(target_layer.as_ref(), other_layer.as_ref(), &candidates, &links.links);
            let symlink = Value::Array(selection.symlink.iter().cloned().map(Value::String).collect());
            let unchanged = match before {
                Some(before @ Value::Array(_)) => &symlink == before,
                _ => selection.symlink.is_empty(),
            };
            let still = selection.still_linked.join(", ");
            if unchanged {
                self.token += 1;
                self.links = None;
                self.view = SettingsView::Main;
                let text = if selection.still_linked.is_empty() {
                    "Linked items unchanged".to_owned()
                } else {
                    format!(
                        "Linked items unchanged; still linked by {}: {still} (edit them in that column)",
                        target_name(other)
                    )
                };
                self.notice = notice(text, !selection.still_linked.is_empty());
                return;
            }
            let target = links.target;
            let change = SettingChange {
                path: setting_path(SettingId::WorktreeLinks, None),
                value: (!selection.symlink.is_empty()).then_some(symlink),
            };
            let extra = if selection.still_linked.is_empty() {
                String::new()
            } else {
                format!(" · still linked by {}: {still}", target_name(other))
            };
            let options = SaveOptions {
                extra,
                ..SaveOptions::default()
            };
            self.save(target, change, "Linked items", options, props, cx);
        }
    }

    /// handleInput in a layer's Actions list.
    fn handle_actions(&mut self, props: &SettingsFlowProps<'_>, input: &Input, cx: &mut FlowCx<App, Self>) {
        let Some(info) = self.info.as_ref() else { return };
        let key = &input.key;
        let column = self.column;
        let id = self.list_id();
        if id == SettingId::DevCommand {
            self.handle_dev_list(props, input, cx);
            return;
        }
        let noun = entry_noun(id);
        let actions = layer_entries(info, id, column).own;
        if let Some(moved) = move_cursor(input, self.action_row, actions.len()) {
            self.action_row = moved;
            self.notice = None;
            return;
        }
        if key.escape {
            self.view = SettingsView::Main;
            self.notice = None;
            return;
        }
        if input.input == "E" {
            self.edit_json(column, props, cx);
            return;
        }
        if input.input == "T" {
            self.review(props, cx);
            return;
        }
        self.notice = None;
        let action = actions.get(self.action_row);
        if input.input == "a" || (key.return_ && action.is_none()) {
            self.edit = Some(SettingsEdit::Text {
                id,
                label: format!("New {noun}"),
                target: column,
                state: EditorState::default(),
                error: None,
                entry: None,
                step: Some(ActionStep::Name),
                placeholder: None,
            });
            return;
        }
        if key.return_
            && let Some(action) = action
        {
            let text = initial_text(&info.targets, id, column, Some(&action.name));
            self.edit = Some(SettingsEdit::Text {
                id,
                label: format!("{} {}", capitalize(noun), action.name),
                target: column,
                state: state_at_end(text),
                error: None,
                entry: Some(action.name.clone()),
                step: None,
                placeholder: None,
            });
            return;
        }
        if input.input == "x"
            && let Some(action) = action
        {
            self.edit = Some(SettingsEdit::Clear {
                id,
                label: format!("{noun} {}", action.name),
                target: column,
                entry: Some(action.name.clone()),
            });
        }
    }

    /// Not in the TS: handleInput in a layer's Dev list: Enter edits a process (or, on + Add a process, adds one),
    /// x removes it.
    fn handle_dev_list(&mut self, props: &SettingsFlowProps<'_>, input: &Input, cx: &mut FlowCx<App, Self>) {
        let Some(info) = self.info.as_ref() else { return };
        let key = &input.key;
        let column = self.column;
        let processes = dev_processes_of(&info.targets, column);
        if let Some(moved) = move_cursor(input, self.action_row, processes.len()) {
            self.action_row = moved;
            self.notice = None;
            return;
        }
        if key.escape {
            self.view = SettingsView::Main;
            self.notice = None;
            return;
        }
        if input.input == "E" {
            self.edit_json(column, props, cx);
            return;
        }
        if input.input == "T" {
            self.review(props, cx);
            return;
        }
        self.notice = None;
        match processes.get(self.action_row) {
            None if key.return_ || input.input == "a" => {
                self.edit = Some(process_edit(column, None, String::new()));
            }
            Some((name, command)) if key.return_ => {
                self.edit = Some(process_edit(column, Some(name.clone()), command.clone()));
            }
            Some((name, _)) if input.input == "x" => {
                self.edit = Some(SettingsEdit::Clear {
                    id: SettingId::DevCommand,
                    label: format!("Dev process {name}"),
                    target: column,
                    entry: Some(name.clone()),
                });
            }
            _ => {}
        }
    }

    /// handleInput on the grid.
    fn handle_grid(&mut self, props: &SettingsFlowProps<'_>, input: &Input, cx: &mut FlowCx<App, Self>) {
        let Some(info) = self.info.as_ref() else { return };
        let key = &input.key;
        if let Some(moved) = move_cursor(input, self.row, SETTINGS.len() - 1) {
            self.row = moved;
            self.notice = None;
            return;
        }
        if key.escape {
            self.close(cx);
            return;
        }
        let column = self.column;
        // ←/→ (Tab toggles): the column; a layer that is unavailable here says why.
        if key.left_arrow || key.right_arrow || key.tab {
            let kind = if key.tab {
                other_target(column)
            } else if key.left_arrow {
                ConfigTargetKind::Global
            } else {
                ConfigTargetKind::Repository
            };
            match column_problem(&info.targets, kind) {
                Some(problem) => self.notice = notice(problem, true),
                None => {
                    self.column = kind;
                    self.notice = None;
                }
            }
            return;
        }
        if input.input == "E" {
            let kind = self.cell_column();
            self.edit_json(kind, props, cx);
            return;
        }
        if input.input == "T" {
            self.review(props, cx);
            return;
        }
        self.notice = None;
        let def = self.def();
        let clear = input.input == "x";
        if !clear && !key.return_ {
            return;
        }
        if is_app_flag(def.id) {
            if clear {
                let how = if def.id == SettingId::ScrollSpeed {
                    "Enter picks a speed"
                } else {
                    "Enter switches it on or off"
                };
                self.notice = notice(format!("{}: {how}", def.label), false);
            } else {
                self.edit = Some(choice_edit(info, def.id, def.label, ConfigTargetKind::Global));
            }
            return;
        }
        if let Some(problem) = column_problem(&info.targets, column) {
            self.notice = notice(problem, true);
            return;
        }
        if clear {
            let layer = info_layer(&info.targets, column);
            let unset = if def.id == SettingId::WorktreeLinks {
                own_value(layer.as_ref(), SettingId::WorktreeLinks, None).is_none()
            } else {
                !layer_sets(layer.as_ref(), def.id, None)
            };
            if unset {
                let files = if def.id == SettingId::WorktreeLinks && layer_sets(layer.as_ref(), def.id, None) {
                    " worktree.files entries are edited in the raw JSON (E)."
                } else {
                    ""
                };
                self.notice = notice(
                    format!(
                        "{} is not set in {}; nothing to clear.{files}",
                        def.label,
                        target_name(column)
                    ),
                    true,
                );
                return;
            }
            self.edit = Some(SettingsEdit::Clear {
                id: def.id,
                label: def.label.to_owned(),
                target: column,
                entry: None,
            });
            return;
        }
        // Not in the TS: the Dev command is edited in the list of what Dev runs (one command, or processes).
        if def.id == SettingId::DevCommand {
            self.open_dev_processes();
            return;
        }
        match def.control {
            SettingControl::Choice => self.edit = Some(choice_edit(info, def.id, def.label, column)),
            SettingControl::Text => {
                let text = initial_text(&info.targets, def.id, column, None);
                let placeholder =
                    inherited_hint(info, def.id, column, None).filter(|placeholder| !placeholder.is_empty());
                self.edit = Some(SettingsEdit::Text {
                    id: def.id,
                    label: def.label.to_owned(),
                    target: column,
                    state: state_at_end(text),
                    error: None,
                    entry: None,
                    step: None,
                    placeholder,
                });
            }
            SettingControl::Actions => {
                self.view = SettingsView::Actions;
                self.action_row = 0;
            }
            SettingControl::Links if info.vars.is_none() => {
                self.notice = notice("Linked items need a Git repository.", true);
            }
            SettingControl::Links => self.open_links(column, props, cx),
        }
    }
}

/// A choice control for `id` in `target`, on its current option.
fn choice_edit(info: &SettingsInfo, id: SettingId, label: &str, target: ConfigTargetKind) -> SettingsEdit {
    let options = choice_options(info, id, target);
    let current = current_choice(info, id, target, &options);
    SettingsEdit::Choice {
        id,
        label: label.to_owned(),
        target,
        index: current.index.unwrap_or(0),
        current: current.current,
        options,
    }
}

impl Flow<App> for SettingsFlow {
    type Props<'a> = SettingsFlowProps<'a>;

    /// TS: SettingsFlow.handleInput (the caller busy-gates it).
    fn handle_input(&mut self, props: &SettingsFlowProps<'_>, input: &Input, cx: &mut FlowCx<App, Self>) {
        let key = &input.key;
        if props.mode == Mode::EditProject {
            self.handle_editor(props, input, cx);
            return;
        }
        if props.mode == Mode::DiscardProject {
            if key.escape {
                cx.emit(FlowEffect::SetMode(Mode::EditProject));
            }
            if key.return_ {
                self.editor_error = None;
                let cwd = self.cwd.clone();
                self.load(&cwd, None, false, props, cx);
            }
            return;
        }
        if self.info.is_none() {
            self.close(cx);
            return;
        }
        if self.edit.is_some() {
            self.handle_edit(input, props, cx);
            return;
        }
        if self.view == SettingsView::Links && self.links.is_some() {
            self.handle_links(props, input, cx);
            return;
        }
        if self.view == SettingsView::Actions {
            self.handle_actions(props, input, cx);
            return;
        }
        self.handle_grid(props, input, cx);
    }

    fn render(&mut self, props: &SettingsFlowProps<'_>, buf: &mut Buffer, area: Rect) {
        if props.mode == Mode::EditProject
            && let Some(document) = self.document.as_ref()
        {
            let pane = ConfigEditorPaneProps {
                document,
                state: &self.editor,
                error: self.editor_error.as_deref(),
            };
            render_config_editor_pane(&pane, &mut self.editor_scroll, buf, area);
            return;
        }
        if props.mode == Mode::DiscardProject {
            let path = self.document.as_ref().map_or("", |document| document.path.as_str());
            let text = format!(
                "{path}\n\nYour draft has unsaved changes. Discarding closes this draft without writing it and returns to Settings; earlier saves are not undone.\n\nEnter discards the draft. Escape returns to editing."
            );
            let footer = [HintPart::from("enter discard · esc keep editing")];
            let pane = DetailsPaneProps {
                title: "Discard unsaved configuration?",
                text: &text,
                footer: &footer,
                scroll: 0,
            };
            render_details_pane(&pane, buf, area);
            return;
        }
        if props.mode != Mode::Settings {
            return;
        }
        let help = self.edit_help();
        let column = self.cell_column();
        let Some(info) = self.info.as_ref() else { return };
        let pane = SettingsPaneProps {
            info,
            view: self.view,
            row: self.row,
            action_row: self.action_row,
            list: self.list_id(),
            column,
            edit: self.edit.as_ref(),
            edit_help: help.as_deref(),
            links: self.links.as_ref(),
            sizes: &self.sizes,
            notice: self.notice.as_ref(),
            grid: deckhand_core::settings_model::settings_grid,
        };
        render_settings_pane(&pane, &mut self.scroll, buf, area);
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use deckhand_client::live::LiveClientHandlers;
    use deckhand_core::key::Key;
    use deckhand_parity::{Case, replay};
    use serde_json::json;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::UnixStream;
    use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
    use tokio::sync::mpsc::UnboundedReceiver;

    use super::*;
    use crate::app::event_loop::Update;
    use crate::app::state::AppProps;
    use crate::app::test_support::app;

    #[test]
    fn is_settings_flow_mode_parity() {
        replay(
            "F4",
            "settingsFlow/isSettingsFlowMode",
            |case: &Case| -> Result<bool, String> {
                // A string with a lone surrogate (not a Rust string) names no mode either.
                Ok(case
                    .arg_str(0)
                    .ok()
                    .and_then(Mode::from_name)
                    .is_some_and(is_settings_flow_mode))
            },
        );
    }

    #[test]
    fn helpers_match_the_ts_ones() {
        assert_eq!(max_text(SettingId::WorktreeBranchName), 200);
        assert_eq!(max_text(SettingId::WorktreeLocation), 4096);
        assert_eq!(max_text(SettingId::DevCommand), 8192);
        assert_eq!(display(None), "undefined");
        assert_eq!(display(Some(&json!([]))), "none");
        assert_eq!(display(Some(&json!([""]))), "none");
        assert_eq!(display(Some(&json!(["local.conf", "a/b"]))), "local.conf, a/b");
        assert_eq!(display(Some(&json!(true))), "on");
        assert_eq!(display(Some(&json!(false))), "off");
        assert_eq!(display(Some(&json!("npm test"))), "npm test");
        assert_eq!(display(Some(&json!(null))), "null");
        assert_eq!(display(Some(&json!(2.5))), "2.5");
        assert_eq!(display(Some(&json!([null, 1, [2, 3]]))), ", 1, 2,3");
        assert_eq!(capitalize("global defaults"), "Global defaults");
        assert_eq!(capitalize("this repo's deckhand.json"), "This repo's deckhand.json");
        assert_eq!(capitalize("ßa"), "SSa");
        assert_eq!(capitalize("😀a"), "😀a");
        assert_eq!(capitalize(""), "");
        for mode in [
            SettingsFlowMode::Settings,
            SettingsFlowMode::EditProject,
            SettingsFlowMode::DiscardProject,
        ] {
            assert!(is_settings_flow_mode(mode.mode()));
        }
        assert!(!is_settings_flow_mode(Mode::Browse));
    }

    /// The settings of `/repo`: a global layer with a `test` action, no deckhand.json yet.
    fn info_json() -> Value {
        json!({
            "cwd": "/repo", "repo": "repo",
            "repository": {"state": "absent", "path": "/repo/deckhand.json"},
            "needsReview": false,
            "rows": [
                {"key": "defaultAgent", "value": "claude", "source": "built-in default"},
                {"key": "devCommand", "value": "dev", "source": "built-in default"},
                {"key": "actions", "entry": "test", "value": "global test", "source": "global", "raw": "global test"},
                {"key": "worktree.hook", "value": "on", "source": "built-in default"}
            ],
            "targets": {
                "global": {"kind": "global", "root": "/r", "path": "/home/me/.deckhand/config.json",
                    "raw": "{\"actions\":{\"test\":\"global test\"}}", "revision": "g1", "exists": true},
                "repository": {"kind": "repository", "root": "/repo", "path": "/repo/deckhand.json",
                    "raw": "{}", "revision": null, "exists": false}
            },
            "vars": {"name": "<name>", "repo": "repo", "repoParent": "/", "repoRoot": "/repo", "home": "/home/me"},
            "defaultLocation": "/state/worktrees/<name>",
            "insideIgnored": false,
            "user": "me",
            "notifications": false
        })
    }

    /// The same settings once deckhand.json has `layer` as its content.
    fn info_with_repository(layer: &Value) -> Value {
        let mut info = info_json();
        info["targets"]["repository"]["raw"] = Value::String(layer.to_string());
        info["targets"]["repository"]["exists"] = Value::Bool(true);
        info["targets"]["repository"]["revision"] = Value::String("r2".to_owned());
        info
    }

    /// The daemon end of the client's socket.
    struct Daemon {
        lines: BufReader<OwnedReadHalf>,
        writer: OwnedWriteHalf,
    }

    impl Daemon {
        async fn next(&mut self) -> Value {
            let mut line = String::new();
            let read = tokio::time::timeout(Duration::from_secs(5), self.lines.read_line(&mut line)).await;
            assert!(matches!(read, Ok(Ok(count)) if count > 0), "a request");
            serde_json::from_str(&line).unwrap_or_else(|error| panic!("{error}"))
        }

        async fn respond(&mut self, request: &Value, body: Value) {
            let mut message = json!({"type": "response", "requestId": request["requestId"]});
            if let (Some(message), Some(body)) = (message.as_object_mut(), body.as_object()) {
                message.extend(body.clone());
            }
            let text = format!("{message}\n");
            self.writer
                .write_all(text.as_bytes())
                .await
                .unwrap_or_else(|error| panic!("{error}"));
        }

        /// Answers the next request (of `kind`) with `data`.
        async fn answer(&mut self, kind: &str, data: Value) -> Value {
            let request = self.next().await;
            assert_eq!(request["type"], kind);
            self.respond(&request, json!({"ok": true, "data": data})).await;
            request
        }
    }

    struct Harness {
        app: App,
        rx: UnboundedReceiver<Update<App>>,
        daemon: Daemon,
    }

    impl Harness {
        fn new() -> Harness {
            let (mut app, rx) = app(AppProps::default());
            let (client, server) = UnixStream::pair().unwrap_or_else(|error| panic!("{error}"));
            let (read, writer) = server.into_split();
            app.connection.client = Some(Rc::new(LiveClient::new(client, LiveClientHandlers::default())));
            Harness {
                app,
                rx,
                daemon: Daemon {
                    lines: BufReader::new(read),
                    writer,
                },
            }
        }

        /// Applies the next update (a settled request's continuation).
        async fn pump(&mut self) {
            let update = tokio::time::timeout(Duration::from_secs(5), self.rx.recv()).await;
            let Ok(Some(update)) = update else {
                panic!("an update");
            };
            update(&mut self.app);
        }

        fn key(&mut self, input: Input) {
            self.app
                .settings_flow_call(|flow, props, cx| flow.handle_input(props, &input, cx));
        }

        fn text(&mut self, text: &str) {
            self.key(Input::text(text));
        }

        fn special(&mut self, key: Key) {
            self.key(Input::key("", key));
        }

        fn flow(&self) -> &SettingsFlow {
            &self.app.flows.settings
        }

        fn cancelable(&mut self) -> bool {
            self.app.settings_flow_call(|flow, props, _cx| flow.cancelable(props))
        }

        /// C on `/repo`, answered with `info`.
        async fn open(&mut self, info: Value) {
            self.app
                .settings_flow_call(|flow, props, cx| flow.open("/repo", props, cx));
            assert!(self.app.messages.busy);
            let request = self.daemon.answer("settings-info", info).await;
            assert_eq!(request["cwd"], "/repo");
            self.pump().await;
        }
    }

    const ENTER: Key = Key {
        return_: true,
        ..Key::NONE
    };
    const ESCAPE: Key = Key {
        escape: true,
        ..Key::NONE
    };

    fn local(body: impl std::future::Future<Output = ()>) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap_or_else(|error| panic!("{error}"));
        tokio::task::LocalSet::new().block_on(&runtime, body);
    }

    fn row_of(id: SettingId) -> usize {
        SETTINGS.iter().position(|def| def.id == id).unwrap_or(0)
    }

    fn edit_text_of(flow: &SettingsFlow) -> Option<(String, Option<String>)> {
        match flow.edit.as_ref() {
            Some(SettingsEdit::Text { state, error, .. } | SettingsEdit::Process { state, error, .. }) => {
                Some((state.text.clone(), error.clone()))
            }
            _ => None,
        }
    }

    #[test]
    fn open_shows_the_settings_and_picks_the_initial_column() {
        local(async {
            let mut h = Harness::new();
            h.app.flows.settings.column = ConfigTargetKind::Global;
            h.app.flows.settings.row = 5;
            h.open(info_json()).await;
            assert_eq!(h.app.mode, Mode::Settings);
            assert!(!h.app.messages.busy);
            let flow = h.flow();
            assert_eq!(flow.cwd, "/repo");
            assert_eq!(flow.row, 0);
            assert_eq!(flow.column, ConfigTargetKind::Repository);
            assert!(flow.info.is_some());
            assert!(flow.token > 0);
        });
    }

    #[test]
    fn grid_keys_move_the_cursor_and_pick_columns() {
        local(async {
            let mut h = Harness::new();
            h.open(info_json()).await;
            h.text("j");
            h.text("j");
            assert_eq!(h.flow().row, 2);
            h.special(Key { end: true, ..Key::NONE });
            assert_eq!(h.flow().row, SETTINGS.len() - 1);
            // A global-only row selects Global whatever the column.
            assert_eq!(h.flow().cell_column(), ConfigTargetKind::Global);
            h.special(Key {
                page_up: true,
                ..Key::NONE
            });
            assert_eq!(h.flow().row, SETTINGS.len() - 6);
            h.special(Key {
                home: true,
                ..Key::NONE
            });
            h.text("k");
            assert_eq!(h.flow().row, 0);
            h.special(Key {
                left_arrow: true,
                ..Key::NONE
            });
            assert_eq!(h.flow().column, ConfigTargetKind::Global);
            h.special(Key { tab: true, ..Key::NONE });
            assert_eq!(h.flow().column, ConfigTargetKind::Repository);
            // x on a setting this layer does not set says so.
            h.text("x");
            assert_eq!(
                h.flow().notice,
                notice(
                    "Default agent is not set in this repo's deckhand.json; nothing to clear.",
                    true
                )
            );
            // An app flag switches with Enter only.
            h.app.flows.settings.row = row_of(SettingId::Notifications);
            h.text("x");
            assert_eq!(
                h.flow().notice,
                notice("Notifications: Enter switches it on or off", false)
            );
            h.special(ENTER);
            assert!(matches!(
                h.flow().edit,
                Some(SettingsEdit::Choice {
                    target: ConfigTargetKind::Global,
                    index: 1,
                    ..
                })
            ));
            assert!(h.cancelable());
            h.special(ESCAPE);
            assert!(h.flow().edit.is_none());
            assert!(!h.cancelable());
            h.special(ESCAPE);
            assert_eq!(h.app.mode, Mode::Browse);
        });
    }

    #[test]
    fn a_choice_saves_into_the_selected_layer() {
        local(async {
            let mut h = Harness::new();
            h.open(info_json()).await;
            h.special(ENTER);
            assert!(matches!(
                h.flow().edit,
                Some(SettingsEdit::Choice {
                    index: 0,
                    current: None,
                    ..
                })
            ));
            // claude is not stored here, so choosing it writes it.
            h.text("j");
            h.text("k");
            h.special(ENTER);
            assert!(h.app.messages.busy);
            let request = h.daemon.next().await;
            assert_eq!(request["type"], "save-config");
            assert_eq!(request["target"], "repository");
            assert_eq!(request["raw"], "{\n  \"defaultAgent\": \"claude\"\n}\n");
            assert_eq!(request["revision"], Value::Null);
            let saved = json!({"kind": "repository", "root": "/repo", "path": "/repo/deckhand.json",
                "raw": request["raw"], "revision": "r2", "exists": true, "trust": "created"});
            h.daemon.respond(&request, json!({"ok": true, "data": saved})).await;
            h.pump().await;
            h.daemon
                .answer(
                    "settings-info",
                    info_with_repository(&json!({"defaultAgent": "claude"})),
                )
                .await;
            h.pump().await;
            assert!(!h.app.messages.busy);
            let expected = saved_message(
                "Default agent",
                ConfigTargetKind::Repository,
                SavedMessageOptions {
                    cleared: false,
                    trust: Some(SaveTrust::Created),
                },
            );
            assert_eq!(h.app.messages.status_message.as_deref(), Some(expected.as_str()));
            assert!(h.flow().edit.is_none());
            // Choosing the stored value again changes nothing.
            h.special(ENTER);
            h.special(ENTER);
            assert!(h.flow().edit.is_none());
            assert_eq!(
                h.flow().notice,
                notice(
                    "Default agent unchanged: this repo's deckhand.json already has it",
                    false
                )
            );
        });
    }

    #[test]
    fn a_text_setting_that_lost_a_race_reloads_and_says_so() {
        local(async {
            let mut h = Harness::new();
            h.open(info_json()).await;
            h.app.flows.settings.row = row_of(SettingId::DevCommand);
            // The Dev command's list, then + Add a process.
            h.special(ENTER);
            h.special(ENTER);
            assert!(matches!(
                h.flow().edit.as_ref(),
                Some(SettingsEdit::Process {
                    step: ProcessStep::Command,
                    name: None,
                    ..
                })
            ));
            // Blank commands are refused in place.
            h.special(ENTER);
            assert_eq!(
                edit_text_of(h.flow()),
                Some((String::new(), Some("Type a command".to_owned())))
            );
            // Pasted newlines are dropped; typing clears the error.
            h.text("npm run\n dev");
            assert_eq!(edit_text_of(h.flow()), Some(("npm run dev".to_owned(), None)));
            h.special(ENTER);
            assert_eq!(edit_text_of(h.flow()), Some(("dev".to_owned(), None)));
            h.special(ENTER);
            let request = h.daemon.next().await;
            assert_eq!(
                request["raw"],
                "{\n  \"devCommand\": {\n    \"dev\": \"npm run dev\"\n  }\n}\n"
            );
            h.daemon
                .respond(
                    &request,
                    json!({"ok": false, "error": "this repo's deckhand.json changed on disk; reload it"}),
                )
                .await;
            h.pump().await;
            h.daemon.answer("settings-info", info_json()).await;
            h.pump().await;
            assert_eq!(
                h.flow().notice,
                notice(
                    "This repo's deckhand.json changed on disk, so Dev process dev was not saved (wanted npm run dev). Reloaded; try again.",
                    true
                )
            );
            assert!(h.flow().edit.is_none());
        });
    }

    #[test]
    fn text_input_keeps_to_the_limit_in_utf16_units() {
        local(async {
            let mut h = Harness::new();
            h.open(info_json()).await;
            h.app.flows.settings.row = row_of(SettingId::WorktreeBranchName);
            h.special(ENTER);
            assert_eq!(
                h.flow().edit_help(),
                Some(vec![line("A template with {name}; {user} is your user name")])
            );
            h.text(&"😀".repeat(150));
            let (text, _) = edit_text_of(h.flow()).unwrap_or_default();
            assert_eq!(js_len(&text), 200);
            let mut flow = SettingsFlow {
                info: h.flow().info.clone(),
                ..SettingsFlow::default()
            };
            flow.edit = Some(SettingsEdit::Text {
                id: SettingId::WorktreeBranchName,
                label: "Branch name".to_owned(),
                target: ConfigTargetKind::Repository,
                state: state_at_end("feat/{name}".to_owned()),
                error: None,
                entry: None,
                step: None,
                placeholder: None,
            });
            assert_eq!(
                flow.edit_help(),
                Some(vec![nowrap("→ feat/my-task for a session named my-task")])
            );
        });
    }

    #[test]
    fn an_action_is_added_name_first_then_its_command() {
        local(async {
            let mut h = Harness::new();
            h.open(info_json()).await;
            h.app.flows.settings.row = row_of(SettingId::Actions);
            h.special(ENTER);
            assert_eq!(h.flow().view, SettingsView::Actions);
            // The global test action is context here: the list's only row is "add".
            h.special(ENTER);
            assert!(matches!(
                h.flow().edit,
                Some(SettingsEdit::Text {
                    step: Some(ActionStep::Name),
                    ..
                })
            ));
            h.special(ENTER);
            assert_eq!(
                edit_text_of(h.flow()),
                Some((String::new(), Some("type a name first".to_owned())))
            );
            h.text("test");
            let help = h.flow().edit_help().unwrap_or_default();
            assert_eq!(
                help.first(),
                Some(&colored("overrides the global test action in this repo", THEME.active))
            );
            assert_eq!(help.len(), 3);
            h.special(ENTER);
            assert!(matches!(
                h.flow().edit.as_ref(),
                Some(SettingsEdit::Text {
                    label,
                    entry: Some(entry),
                    step: Some(ActionStep::Command),
                    placeholder: Some(placeholder),
                    ..
                }) if label == "Action test" && entry == "test" && placeholder == "global: global test"
            ));
            h.text("make test");
            h.special(ENTER);
            let request = h.daemon.next().await;
            assert_eq!(
                request["raw"],
                "{\n  \"actions\": {\n    \"test\": \"make test\"\n  }\n}\n"
            );
            let saved = json!({"kind": "repository", "root": "/repo", "path": "/repo/deckhand.json",
                "raw": request["raw"], "revision": "r2", "exists": true, "trust": "created"});
            h.daemon.respond(&request, json!({"ok": true, "data": saved})).await;
            h.pump().await;
            h.daemon
                .answer(
                    "settings-info",
                    info_with_repository(&json!({"actions": {"test": "make test"}})),
                )
                .await;
            h.pump().await;
            let expected = action_saved_message("test", ConfigTargetKind::Repository, Some(SaveTrust::Created));
            assert_eq!(h.app.messages.status_message.as_deref(), Some(expected.as_str()));
            assert_eq!(h.flow().action_row, 0);
            assert_eq!(h.flow().view, SettingsView::Actions);
            // x asks before clearing it.
            h.text("x");
            assert!(matches!(
                h.flow().edit.as_ref(),
                Some(SettingsEdit::Clear { label, entry: Some(entry), .. }) if label == "action test" && entry == "test"
            ));
            h.special(ESCAPE);
            h.special(ESCAPE);
            assert_eq!(h.flow().view, SettingsView::Main);
        });
    }

    #[test]
    fn dev_processes_are_added_and_edited_in_two_steps() {
        local(async {
            let mut h = Harness::new();
            h.open(info_with_repository(&json!({}))).await;
            h.app.flows.settings.row = row_of(SettingId::DevCommand);
            // Enter lists what Dev runs; with nothing yet, only + Add a process.
            h.special(ENTER);
            assert_eq!(h.flow().view, SettingsView::Actions);
            assert_eq!(h.flow().list_id(), SettingId::DevCommand);
            let saved = |request: &Value, revision: &str| {
                json!({"kind": "repository", "root": "/repo", "path": "/repo/deckhand.json",
                    "raw": request["raw"], "revision": revision, "exists": true, "trust": "kept"})
            };
            // Step 1 the command, step 2 the name suggested from it: Enter, Enter.
            h.special(ENTER);
            h.text("cd backend && python3 main.py");
            h.special(ENTER);
            assert!(matches!(
                h.flow().edit,
                Some(SettingsEdit::Process {
                    step: ProcessStep::Name,
                    name: None,
                    ..
                })
            ));
            assert_eq!(edit_text_of(h.flow()), Some(("backend".to_owned(), None)));
            h.special(ENTER);
            let request = h.daemon.next().await;
            assert_eq!(
                request["raw"],
                "{\n  \"devCommand\": {\n    \"backend\": \"cd backend && python3 main.py\"\n  }\n}\n"
            );
            h.daemon
                .respond(&request, json!({"ok": true, "data": saved(&request, "r2")}))
                .await;
            h.pump().await;
            let one = json!({"devCommand": {"backend": "cd backend && python3 main.py"}});
            h.daemon.answer("settings-info", info_with_repository(&one)).await;
            h.pump().await;
            let expected = dev_process_saved_message("backend", ConfigTargetKind::Repository, Some(SaveTrust::Kept));
            assert_eq!(h.app.messages.status_message.as_deref(), Some(expected.as_str()));

            // A second one; its suggested name can be changed before Enter.
            h.text("j");
            h.special(ENTER);
            h.text("cd frontend && npm run dev");
            h.special(ENTER);
            assert_eq!(edit_text_of(h.flow()), Some(("frontend".to_owned(), None)));
            h.text("-app");
            h.special(ENTER);
            let request = h.daemon.next().await;
            assert_eq!(
                request["raw"],
                "{\n  \"devCommand\": {\n    \"backend\": \"cd backend && python3 main.py\",\n    \"frontend-app\": \"cd frontend && npm run dev\"\n  }\n}\n"
            );
            h.daemon
                .respond(&request, json!({"ok": true, "data": saved(&request, "r3")}))
                .await;
            h.pump().await;
            let two = json!({"devCommand": {"backend": "cd backend && python3 main.py", "frontend-app": "cd frontend && npm run dev"}});
            h.daemon.answer("settings-info", info_with_repository(&two)).await;
            h.pump().await;
            assert_eq!(h.flow().action_row, 1);

            // Editing walks the same two steps from what is saved: Enter, Enter changes nothing.
            h.text("k");
            h.special(ENTER);
            assert_eq!(
                edit_text_of(h.flow()),
                Some(("cd backend && python3 main.py".to_owned(), None))
            );
            h.special(ENTER);
            assert_eq!(edit_text_of(h.flow()), Some(("backend".to_owned(), None)));
            h.special(ENTER);
            assert_eq!(h.flow().notice, notice("Dev process backend unchanged", false));
            // A taken name is refused in place.
            let info: SettingsInfo = serde_json::from_value(info_with_repository(&two)).unwrap();
            assert_eq!(
                dev_name_problem(
                    &info.targets,
                    ConfigTargetKind::Repository,
                    "frontend-app",
                    Some("backend")
                ),
                Some("frontend-app is already a Dev process here".to_owned())
            );
        });
    }

    #[test]
    fn the_link_picker_loads_sizes_and_guards_unsaved_changes() {
        local(async {
            let mut h = Harness::new();
            h.open(info_json()).await;
            h.app.flows.settings.row = row_of(SettingId::WorktreeLinks);
            h.special(ENTER);
            let candidates = json!({
                "checkout": "/repo",
                "candidates": [
                    {"path": "local.conf", "kind": "file", "ignored": true, "suggestion": "link", "reason": "conf"},
                    {"path": "node_modules", "kind": "dir", "ignored": true, "suggestion": "skip", "reason": "big"},
                    {"path": "gone", "kind": "file", "ignored": true, "suggestion": "skip", "reason": "", "missing": true}
                ],
                "moreCandidates": 0
            });
            h.daemon.answer("worktree-candidates", candidates).await;
            h.pump().await;
            assert_eq!(h.flow().view, SettingsView::Links);
            let sizes = json!({"local.conf": 1, "node_modules": 900});
            let request = h.daemon.answer("worktree-candidate-sizes", sizes).await;
            assert_eq!(request["paths"], json!(["local.conf", "node_modules"]));
            h.pump().await;
            assert_eq!(h.flow().sizes.get("node_modules"), Some(&Some(900)));
            // The suggestion is preselected, which is not a change.
            assert!(!h.flow().links_dirty());
            assert!(!h.cancelable());
            h.text("j");
            h.text(" ");
            assert!(h.flow().links_dirty());
            assert!(h.cancelable());
            h.text("E");
            assert_eq!(
                h.flow().notice,
                notice(
                    "Unsaved link changes: Enter saves them (Esc discards) before E opens the raw JSON.",
                    true
                )
            );
            // a adds a path after the configured rows (none here: a suggestion is not one).
            h.text("a");
            h.text("config/local.json/");
            h.special(ENTER);
            let links = h.flow().links.clone().unwrap_or_else(|| panic!("links"));
            let paths: Vec<&str> = links
                .data
                .candidates
                .iter()
                .map(|candidate| candidate.path.as_str())
                .collect();
            assert_eq!(paths, ["config/local.json", "local.conf", "node_modules", "gone"]);
            assert_eq!(links.row, 0);
            assert_eq!(links.links.get("config/local.json"), Some(&true));
            h.text("a");
            h.text("../x");
            h.special(ENTER);
            assert_eq!(
                edit_text_of(h.flow()).and_then(|(_, error)| error).as_deref(),
                Some("A path relative to the repository root (no leading /, no ..)")
            );
            h.special(ESCAPE);
            h.special(ESCAPE);
            assert_eq!(h.flow().edit, Some(SettingsEdit::DiscardLinks));
            h.special(ENTER);
            assert!(h.flow().links.is_none());
            assert_eq!(h.flow().view, SettingsView::Main);
        });
    }

    #[test]
    fn the_raw_json_editor_formats_and_asks_before_discarding() {
        local(async {
            let mut h = Harness::new();
            h.open(info_json()).await;
            h.text("E");
            h.daemon.answer("settings-info", info_json()).await;
            h.pump().await;
            assert_eq!(h.app.mode, Mode::EditProject);
            assert_eq!(h.flow().editor.text, "{}");
            assert!(h.cancelable());
            h.text("x");
            h.key(Input::key(
                "f",
                Key {
                    ctrl: true,
                    ..Key::NONE
                },
            ));
            assert!(h.flow().editor_error.is_some());
            h.special(ESCAPE);
            assert_eq!(h.app.mode, Mode::DiscardProject);
            let area = Rect::new(0, 0, 60, 12);
            let mut buf = Buffer::empty(area);
            let props = SettingsFlowProps {
                client: None,
                mode: Mode::DiscardProject,
            };
            h.app.flows.settings.render(&props, &mut buf, area);
            let screen: String = buf.content().iter().map(|cell| cell.symbol()).collect();
            assert!(screen.contains("Discard unsaved configuration?"));
            h.special(ESCAPE);
            assert_eq!(h.app.mode, Mode::EditProject);
            h.special(ESCAPE);
            h.special(ENTER);
            h.daemon.answer("settings-info", info_json()).await;
            h.pump().await;
            assert_eq!(h.app.mode, Mode::Settings);
        });
    }
}
