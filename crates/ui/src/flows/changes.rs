//! Port of changesFlow.tsx: the Git tab's Changes view. Selection (kept by group + path across refreshes), the
//! selected entry's diff, and the focus-mode keys (→). The record itself is watched by the app like the other panes
//! (watch-changes, changes-updated).

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use deckhand_client::live::{LiveClient, StageEntry};
use deckhand_core::changes_model::{PreviousSelection, change_key, change_rows, group_offset, reselect, stage_mode};
use deckhand_core::display::error_message;
use deckhand_core::jsstr::path;
use deckhand_core::key::Input;
use deckhand_protocol::data::{ChangeEntry, ChangeGroup, ChangesRecord, SessionRecord, StageMode};
use deckhand_sys::async_util::{Task, Timer};
use deckhand_sys::desktop::open_in_editor;

use crate::app::state::App;
use crate::flow::{Flow, FlowCx, FlowEffect};
use crate::keymap::{Binding, HelpRef, KeySpec, Keymap, NamedKey};
use crate::panes::changes::{ChangesPaneProps, DiffState, changes_layout, diff_lines, render_changes_pane};

/// TS: changesFlow.tsx DIFF_DEBOUNCE_MS: the selected entry's diff is fetched this long after the selection settles.
pub const DIFF_DEBOUNCE_MS: u64 = 60;

/// TS: changesFlow.tsx DIFF_STEP: J/K scroll the diff by this many lines.
pub const DIFF_STEP: usize = 3;

/// TS: changesFlow.tsx plural.
pub fn plural(count: i64, word: &str) -> String {
    format!("{count} {word}{}", if count == 1 { "" } else { "s" })
}

/// TS: changesFlow.tsx ChangesFlowOptions (values; `onChanges`, `onExit`, `onAttach`, `setBusy`, `setError` and
/// `setStatusMessage` are effects).
#[derive(Clone, Copy, Debug)]
pub struct ChangesFlowProps<'a> {
    pub client: Option<&'a Rc<LiveClient>>,
    pub session: Option<&'a SessionRecord>,
    pub changes: &'a ChangesRecord,
    pub focused: bool,
}

/// TS: changesFlow.tsx useChangesFlow's state.
#[derive(Debug)]
pub struct ChangesFlow {
    /// TS `selection`: the selected entry's key, index and position in its group.
    pub selection: PreviousSelection,
    pub diff: Option<DiffState>,
    pub diff_scroll: usize,
    /// TS `viewport.current.diffRows`: the diff's rows at the last render (10 before one).
    pub diff_rows: usize,
    /// The session the selection belongs to (TS: the `[session?.id]` effect resets it).
    pub session_id: Option<String>,
    /// The entry key the diff scroll belongs to (TS: the `[key]` effect resets it).
    pub diff_scroll_key: Option<String>,
    /// The debounced diff fetch and its request (TS: the effect's timer and `cancelled`).
    pub diff_timer: Option<Timer>,
    pub diff_request: Option<Task<()>>,
    /// What the diff was last requested for: session, key and record (TS: the effect's dependencies).
    pub diff_for: Option<(String, String, ChangesRecord)>,
    /// The client `diff_for` was requested with (TS: the effect's `client` dependency; its address).
    pub diff_client: usize,
    /// The `[key, selected]` the follow-the-selection effect last ran for (`None` before its first run).
    pub follow_deps: Option<(Option<String>, Option<usize>)>,
}

impl Default for ChangesFlow {
    fn default() -> Self {
        ChangesFlow {
            selection: PreviousSelection::default(),
            diff: None,
            diff_scroll: 0,
            diff_rows: 10,
            session_id: None,
            diff_scroll_key: None,
            diff_timer: None,
            diff_request: None,
            diff_for: None,
            diff_client: 0,
            follow_deps: None,
        }
    }
}

/// The guide topic that documents the Changes keys.
const GUIDE: HelpRef = HelpRef::Topic("Git & PRs");

/// The Changes focus keys.
pub static KEYMAP: Keymap = &[
    Binding {
        keys: &[KeySpec::Named(NamedKey::Escape), KeySpec::Named(NamedKey::Left)],
        action: "back to the session list",
        help: GUIDE,
    },
    Binding {
        keys: &[KeySpec::Char("h")],
        action: "back to the session list",
        help: HelpRef::Hidden("h mirrors the left arrow and Esc"),
    },
    Binding {
        keys: &[KeySpec::Char("o")],
        action: "attach lazygit",
        help: GUIDE,
    },
    Binding {
        keys: &[KeySpec::Char("j"), KeySpec::Named(NamedKey::Down)],
        action: "select the next entry",
        help: GUIDE,
    },
    Binding {
        keys: &[KeySpec::Char("k"), KeySpec::Named(NamedKey::Up)],
        action: "select the previous entry",
        help: GUIDE,
    },
    Binding {
        keys: &[KeySpec::Char("g"), KeySpec::Named(NamedKey::Home)],
        action: "select the first entry",
        help: GUIDE,
    },
    Binding {
        keys: &[KeySpec::Char("G"), KeySpec::Named(NamedKey::End)],
        action: "select the last entry",
        help: GUIDE,
    },
    Binding {
        keys: &[KeySpec::Char("J")],
        action: "scroll the diff down",
        help: GUIDE,
    },
    Binding {
        keys: &[KeySpec::Char("K")],
        action: "scroll the diff up",
        help: GUIDE,
    },
    Binding {
        keys: &[KeySpec::Named(NamedKey::PageDown)],
        action: "scroll the diff a page down",
        help: GUIDE,
    },
    Binding {
        keys: &[KeySpec::Named(NamedKey::PageUp)],
        action: "scroll the diff a page up",
        help: GUIDE,
    },
    Binding {
        keys: &[KeySpec::Char(" ")],
        action: "stage or unstage the entry",
        help: GUIDE,
    },
    Binding {
        keys: &[KeySpec::Char("a")],
        action: "stage everything",
        help: GUIDE,
    },
    Binding {
        keys: &[KeySpec::Char("A")],
        action: "unstage everything",
        help: GUIDE,
    },
    Binding {
        keys: &[KeySpec::Named(NamedKey::Return), KeySpec::Char("E")],
        action: "open the file in the editor",
        help: GUIDE,
    },
];

/// The editor launcher (TS `openInEditor`; tests pass a stand-in so no editor starts).
type Opener = fn(&str, &dyn Fn(String), Option<i64>) -> Option<String>;

fn to_isize(value: usize) -> isize {
    isize::try_from(value).unwrap_or(isize::MAX)
}

impl ChangesFlow {
    /// The record the flow works on: the selected session's, with a workspace (TS `current`).
    pub fn current<'a>(props: &ChangesFlowProps<'a>) -> Option<&'a ChangesRecord> {
        let changes = props.changes;
        let same_session = changes.session_id.as_deref() == props.session.map(|session| session.id.as_str());
        let has_workspace = changes
            .workspace
            .as_deref()
            .is_some_and(|workspace| !workspace.is_empty());
        (same_session && has_workspace).then_some(changes)
    }

    /// The entries of `current()` (TS `entries`).
    fn entries<'a>(props: &ChangesFlowProps<'a>) -> &'a [ChangeEntry] {
        Self::current(props)
            .map(|current| current.entries.as_slice())
            .unwrap_or_default()
    }

    /// The selected entry's index in `current().entries` (TS `selected`, via reselect).
    pub fn selected(&self, props: &ChangesFlowProps<'_>) -> Option<usize> {
        reselect(Self::entries(props), &self.selection)
    }

    /// The selected entry and its key (TS `entry`, `key`).
    fn entry_and_key<'a>(&self, props: &ChangesFlowProps<'a>) -> Option<(&'a ChangeEntry, String)> {
        let entry = self.selected(props).and_then(|index| Self::entries(props).get(index))?;
        Some((entry, change_key(entry)))
    }

    /// Cancels the debounced fetch and the request in flight (TS the diff effect's cleanup).
    fn cancel_diff(&mut self) {
        if let Some(mut timer) = self.diff_timer.take() {
            timer.clear();
        }
        if let Some(request) = self.diff_request.take() {
            request.abort();
        }
    }

    /// TS: useChangesFlow's effects: follow the selected entry, reset on a new session or entry, and (focused)
    /// fetch the selected entry's diff after DIFF_DEBOUNCE_MS when the record or selection changed.
    pub fn sync(&mut self, props: &ChangesFlowProps<'_>, cx: &mut FlowCx<App, Self>) {
        // React renders again after an effect set state and runs the effects whose dependencies changed; this is
        // that, until no effect sets state (the session reset's cascade settles within a few passes).
        for _ in 0..4 {
            let selected = self.selected(props);
            let key = self.entry_and_key(props).map(|(_, key)| key);
            let mut changed = false;

            // Follow the entry that is now selected (e.g. the next one after staging the last selection away).
            let deps = (key.clone(), selected);
            if self.follow_deps.as_ref() != Some(&deps) {
                self.follow_deps = Some(deps);
                if key != self.selection.key || selected != Some(self.selection.index) {
                    self.selection = PreviousSelection {
                        key: key.clone(),
                        index: selected.unwrap_or(0),
                        offset: Some(selected.map_or(0, |index| group_offset(Self::entries(props), index))),
                    };
                    changed = true;
                }
            }
            if self.diff_scroll_key != key {
                self.diff_scroll_key = key;
                if self.diff_scroll != 0 {
                    self.diff_scroll = 0;
                    changed = true;
                }
            }
            let session_id = props.session.map(|session| session.id.clone());
            if self.session_id != session_id {
                self.session_id = session_id;
                self.selection = PreviousSelection::default();
                self.diff = None;
                changed = true;
            }
            if !changed {
                break;
            }
        }
        self.sync_diff(props, cx);
    }

    /// TS: the diff effect: the selected entry's diff, while focused; refetched when the record changes (compared
    /// by value: the daemon pushes only on changes). Its deps: client, focused, session, key and record.
    fn sync_diff(&mut self, props: &ChangesFlowProps<'_>, cx: &mut FlowCx<App, Self>) {
        let wanted = match (
            props.client,
            props.session,
            self.entry_and_key(props),
            Self::current(props),
        ) {
            (Some(client), Some(session), Some((entry, key)), Some(current)) if props.focused => {
                Some((client, session, entry, key, current))
            }
            _ => None,
        };
        let Some((client, session, entry, key, current)) = wanted else {
            self.cancel_diff();
            self.diff_for = None;
            return;
        };
        let address = Rc::as_ptr(client) as usize;
        let same = self.diff_client == address
            && matches!(&self.diff_for, Some((last_session, last_key, last)) if *last_session == session.id && *last_key == key && last == current);
        if same {
            return;
        }
        self.cancel_diff();
        self.diff_client = address;
        self.diff_for = Some((session.id.clone(), key.clone(), current.clone()));
        let client = Rc::clone(client);
        let session_id = session.id.clone();
        let (group, entry_path) = (entry.group, entry.path.clone());
        self.diff_timer = Some(cx.after(Duration::from_millis(DIFF_DEBOUNCE_MS), move |flow, cx| {
            let reply = client.changes_diff(&session_id, group, &entry_path);
            flow.diff_request = Some(cx.spawn(reply, move |flow, result, _cx| {
                flow.diff_request = None;
                flow.diff = Some(match result {
                    Ok(next) => DiffState {
                        key,
                        diff: Some(next),
                        error: None,
                    },
                    Err(error) => DiffState {
                        key,
                        diff: None,
                        error: Some(error_message(&error)),
                    },
                });
            }));
        }));
    }

    /// TS: useChangesFlow move: select entry `index` (clamped).
    pub fn move_to(&mut self, index: isize, props: &ChangesFlowProps<'_>) {
        let entries = Self::entries(props);
        let Some(last) = entries.len().checked_sub(1) else {
            return;
        };
        let next = usize::try_from(index).unwrap_or(0).min(last);
        let Some(entry) = entries.get(next) else { return };
        self.selection = PreviousSelection {
            key: Some(change_key(entry)),
            index: next,
            offset: Some(group_offset(entries, next)),
        };
    }

    /// TS: useChangesFlow scrollDiff.
    pub fn scroll_diff(&mut self, delta: isize) {
        // TS `diff?.key === key`: the diff belongs to the selected entry (`selection.key` follows it in `sync`).
        let lines = match (&self.diff, &self.selection.key) {
            (Some(diff), Some(key)) if diff.key == *key => diff_lines(diff.diff.as_ref()).len(),
            _ => 0,
        };
        let limit = to_isize(lines.saturating_sub(self.diff_rows));
        let target = to_isize(self.diff_scroll).saturating_add(delta).clamp(0, limit);
        self.diff_scroll = usize::try_from(target).unwrap_or(0);
    }

    /// TS: useChangesFlow stage: stage/unstage one entry, or everything without `target`.
    pub fn stage(
        &mut self,
        mode: StageMode,
        target: Option<&ChangeEntry>,
        props: &ChangesFlowProps<'_>,
        cx: &mut FlowCx<App, Self>,
    ) {
        let (Some(client), Some(session)) = (props.client, props.session) else {
            return;
        };
        cx.set_busy(true);
        cx.set_error(None);
        let entry = target.map(|target| StageEntry {
            group: target.group,
            path: target.path.clone(),
        });
        let target_path = target.map(|target| target.path.clone());
        let reply = client.change_stage(&session.id, mode, entry);
        // `void`: the work outlives this call and reports through the continuation.
        drop(cx.spawn(reply, move |_flow, result, cx| {
            match result {
                Ok(result) => {
                    let message = if target_path.is_none() && result.changed == 0 {
                        format!("Nothing to {mode}")
                    } else {
                        let what = target_path.unwrap_or_else(|| plural(result.changed, "file"));
                        let verb = if mode == StageMode::Stage { "Staged" } else { "Unstaged" };
                        let left = if result.skipped_conflicts == 0 {
                            String::new()
                        } else {
                            format!(
                                " · {} left: stage each with space once resolved",
                                plural(result.skipped_conflicts, "conflicted file")
                            )
                        };
                        format!("{verb} {what}{left}")
                    };
                    cx.emit(FlowEffect::Changes(Box::new(result.changes)));
                    cx.set_status(message);
                }
                Err(error) => cx.set_error(error_message(&error)),
            }
            cx.set_busy(false);
        }));
    }

    /// TS: useChangesFlow open: the file in the editor at its first changed line (a deleted file cannot be opened).
    pub fn open(&mut self, props: &ChangesFlowProps<'_>, cx: &mut FlowCx<App, Self>) {
        self.open_with(props, cx, open_in_editor);
    }

    fn open_with(&mut self, props: &ChangesFlowProps<'_>, cx: &mut FlowCx<App, Self>, opener: Opener) {
        let (Some(client), Some(session)) = (props.client, props.session) else {
            return;
        };
        let Some((entry, key)) = self.entry_and_key(props) else {
            return;
        };
        let Some(workspace) = Self::current(props).and_then(|current| current.workspace.as_deref()) else {
            return;
        };
        let file = path::join(&[workspace, &entry.path]);
        if !std::path::Path::new(&file).exists() {
            cx.set_status(format!("{} is deleted; nothing to open (o opens lazygit)", entry.path));
            return;
        }
        let entry_path = entry.path.clone();
        if entry.group == ChangeGroup::Untracked {
            open_at(&file, &entry_path, 1, opener, cx);
            return;
        }
        let shown = self
            .diff
            .as_ref()
            .filter(|diff| diff.key == key)
            .and_then(|diff| diff.diff.as_ref());
        if let Some(loaded) = shown {
            open_at(&file, &entry_path, loaded.first_line, opener, cx);
            return;
        }
        // `.catch(() => undefined)`: a failed fetch opens at line 1.
        let reply = client.changes_diff(&session.id, entry.group, &entry.path);
        drop(cx.spawn(async move { reply.await.ok() }, move |_flow, loaded, cx| {
            let line = loaded.map_or(1, |loaded| loaded.first_line);
            open_at(&file, &entry_path, line, opener, cx);
        }));
    }
}

/// The end of TS `open`: starts the editor at `line`, then reports it (or the error).
fn open_at(file: &str, entry_path: &str, line: i64, opener: Opener, cx: &mut FlowCx<App, ChangesFlow>) {
    let errors = RefCell::new(Vec::new());
    let label = opener(file, &|message| errors.borrow_mut().push(message), Some(line));
    for message in errors.into_inner() {
        cx.set_error(message);
    }
    if let Some(label) = label {
        cx.set_status(format!("Opened {entry_path}:{line} in {label}"));
    }
}

impl Flow<App> for ChangesFlow {
    type Props<'a> = ChangesFlowProps<'a>;

    fn handle_input(&mut self, props: &ChangesFlowProps<'_>, input: &Input, cx: &mut FlowCx<App, Self>) {
        let key = &input.key;
        let text = input.input.as_str();
        if key.escape || key.left_arrow || text == "h" {
            // Leaving focus ends the diff effect (TS: `focused` turns false).
            self.cancel_diff();
            self.diff_for = None;
            cx.exit();
            return;
        }
        if text == "o" {
            cx.emit(FlowEffect::AttachGit);
            return;
        }
        // TS `selected` is -1 without a selection.
        let selected = self.selected(props).map_or(-1, to_isize);
        if key.down_arrow || text == "j" {
            self.move_to(selected.saturating_add(1), props);
            return;
        }
        if key.up_arrow || text == "k" {
            self.move_to(selected.saturating_sub(1), props);
            return;
        }
        if key.home || text == "g" {
            self.move_to(0, props);
            return;
        }
        if key.end || text == "G" {
            self.move_to(to_isize(Self::entries(props).len()).saturating_sub(1), props);
            return;
        }
        if text == "J" {
            self.scroll_diff(to_isize(DIFF_STEP));
            return;
        }
        if text == "K" {
            self.scroll_diff(-to_isize(DIFF_STEP));
            return;
        }
        let page = to_isize(self.diff_rows.saturating_sub(1).max(1));
        if key.page_down {
            self.scroll_diff(page);
            return;
        }
        if key.page_up {
            self.scroll_diff(-page);
            return;
        }
        if Self::current(props).is_none() {
            return;
        }
        if text == " " {
            if let Some((entry, _)) = self.entry_and_key(props) {
                self.stage(stage_mode(entry), Some(entry), props, cx);
            }
            return;
        }
        if text == "a" {
            self.stage(StageMode::Stage, None, props, cx);
            return;
        }
        if text == "A" {
            self.stage(StageMode::Unstage, None, props, cx);
            return;
        }
        if key.return_ || text == "E" {
            self.open(props, cx);
        }
    }

    fn render(&mut self, props: &ChangesFlowProps<'_>, buf: &mut Buffer, area: Rect) {
        let rows = Self::current(props).map_or(0, |current| change_rows(current).len());
        self.diff_rows = changes_layout(usize::from(area.width), usize::from(area.height), rows).diff_rows;
        let selected = self.selected(props);
        let key = self.entry_and_key(props).map(|(_, key)| key);
        let diff = self.diff.as_ref().filter(|diff| Some(&diff.key) == key.as_ref());
        let pane = ChangesPaneProps {
            session: props.session,
            changes: props.changes,
            focused: props.focused,
            selected,
            diff,
            diff_scroll: self.diff_scroll,
        };
        render_changes_pane(&pane, buf, area);
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::time::Duration;

    use serde_json::{Value, json};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::UnixStream;
    use tokio::sync::mpsc::UnboundedReceiver;
    use tokio::task::LocalSet;

    use super::*;
    use crate::app::event_loop::Update;
    use crate::app::state::AppProps;
    use crate::app::test_support::{app, session};
    use crate::flow::FlowHost;
    use crate::keymap::{undocumented_in_topic, unknown_topics};
    use deckhand_core::key::Key;
    use deckhand_protocol::data::SessionStatus;

    type Requests = Rc<RefCell<Vec<Value>>>;

    thread_local! {
        static OPENED: RefCell<Vec<(String, Option<i64>)>> = const { RefCell::new(Vec::new()) };
    }

    fn fake_opener(target: &str, _on_error: &dyn Fn(String), line: Option<i64>) -> Option<String> {
        OPENED.with(|opened| opened.borrow_mut().push((target.to_owned(), line)));
        Some("Code".to_owned())
    }

    fn failing_opener(_target: &str, on_error: &dyn Fn(String), _line: Option<i64>) -> Option<String> {
        on_error("could not find cursor or code command on PATH".to_owned());
        None
    }

    fn opened() -> Vec<(String, Option<i64>)> {
        OPENED.with(|opened| opened.borrow().clone())
    }

    /// A client whose daemon answers every request with `reply(request)` (`Err` is a failed response).
    fn fake_daemon(reply: impl Fn(&Value) -> Result<Value, String> + 'static) -> (Rc<LiveClient>, Requests) {
        let (client_side, server_side) = UnixStream::pair().unwrap_or_else(|error| panic!("{error}"));
        let client = Rc::new(LiveClient::new(client_side, Default::default()));
        let requests: Requests = Rc::default();
        let seen = Rc::clone(&requests);
        tokio::task::spawn_local(async move {
            let (read, mut write) = server_side.into_split();
            let mut lines = BufReader::new(read).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let Ok(request) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                seen.borrow_mut().push(request.clone());
                let id = request.get("requestId").cloned().unwrap_or(Value::Null);
                let response = match reply(&request) {
                    Ok(data) => json!({"type": "response", "requestId": id, "ok": true, "data": data}),
                    Err(error) => json!({"type": "response", "requestId": id, "ok": false, "error": error}),
                };
                if write.write_all(format!("{response}\n").as_bytes()).await.is_err() {
                    break;
                }
            }
        });
        (client, requests)
    }

    fn entry(group: &str, path: &str, status: &str) -> Value {
        json!({"group": group, "path": path, "status": status})
    }

    fn record(session_id: &str, workspace: Option<&str>, entries: &[Value]) -> ChangesRecord {
        let count = |group: &str| entries.iter().filter(|entry| entry["group"] == group).count();
        serde_json::from_value(json!({
            "loaded": true,
            "entries": entries,
            "counts": {
                "conflicts": count("conflicts"), "staged": count("staged"),
                "unstaged": count("unstaged"), "untracked": count("untracked"),
            },
            "omitted": {"conflicts": 0, "staged": 0, "unstaged": 0, "untracked": 0},
            "branch": "main",
            "sessionId": session_id,
            "workspace": workspace,
        }))
        .unwrap_or_else(|error| panic!("{error}"))
    }

    fn diff_json(path: &str, first_line: i64) -> Value {
        json!({
            "group": "unstaged", "path": path, "text": "@@ -1 +1 @@\n-a\n+b\n",
            "truncated": false, "binary": false, "firstLine": first_line,
        })
    }

    fn files(paths: &[(&str, &str, &str)]) -> Vec<Value> {
        paths
            .iter()
            .map(|(group, path, status)| entry(group, path, status))
            .collect()
    }

    fn press(text: &str) -> Input {
        Input::text(text)
    }

    fn named(key: Key) -> Input {
        Input::key("", key)
    }

    /// Calls `body` on the app's Changes flow with a fresh cx; the effects it emitted come back (not applied).
    fn call<R>(
        app: &mut App,
        body: impl FnOnce(&mut ChangesFlow, &mut FlowCx<App, ChangesFlow>) -> R,
    ) -> (R, Vec<FlowEffect<App>>) {
        let mut cx = FlowCx::new(app.dispatch().clone(), |app: &mut App| &mut app.flows.changes);
        let result = body(&mut app.flows.changes, &mut cx);
        let effects = cx.take_effects();
        (result, effects)
    }

    fn debug(effects: &[FlowEffect<App>]) -> Vec<String> {
        effects.iter().map(|effect| format!("{effect:?}")).collect()
    }

    async fn pump(app: &mut App, rx: &mut UnboundedReceiver<Update<App>>, updates: usize) {
        for _ in 0..updates {
            let update = tokio::time::timeout(Duration::from_secs(5), rx.recv())
                .await
                .unwrap_or_else(|_| panic!("no update arrived"))
                .unwrap_or_else(|| panic!("update channel closed"));
            update(app);
        }
    }

    /// Runs `body` on a current-thread runtime inside a LocalSet.
    fn local(body: impl std::future::Future<Output = ()>) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap_or_else(|error| panic!("{error}"));
        LocalSet::new().block_on(&runtime, body);
    }

    fn selected_path(flow: &ChangesFlow, props: &ChangesFlowProps<'_>) -> Option<String> {
        flow.selected(props)
            .and_then(|index| props.changes.entries.get(index))
            .map(|entry| entry.path.clone())
    }

    fn props<'a>(
        client: Option<&'a Rc<LiveClient>>,
        session: &'a SessionRecord,
        changes: &'a ChangesRecord,
        focused: bool,
    ) -> ChangesFlowProps<'a> {
        ChangesFlowProps {
            client,
            session: Some(session),
            changes,
            focused,
        }
    }

    #[test]
    fn plural_adds_an_s_unless_one() {
        assert_eq!(plural(1, "file"), "1 file");
        assert_eq!(plural(0, "file"), "0 files");
        assert_eq!(plural(2, "conflicted file"), "2 conflicted files");
    }

    #[test]
    fn the_flow_works_on_the_selected_sessions_record_with_a_workspace() {
        let s1 = session("s1", "One", SessionStatus::Running);
        let entries = files(&[("unstaged", "a.txt", "M")]);
        let good = record("s1", Some("/repo"), &entries);
        assert!(ChangesFlow::current(&props(None, &s1, &good, true)).is_some());
        let other = record("s2", Some("/repo"), &entries);
        assert!(ChangesFlow::current(&props(None, &s1, &other, true)).is_none());
        let bare = record("s1", None, &entries);
        assert!(ChangesFlow::current(&props(None, &s1, &bare, true)).is_none());
        let empty = record("s1", Some(""), &entries);
        assert!(ChangesFlow::current(&props(None, &s1, &empty, true)).is_none());
        // `undefined === undefined`: no session and a record without a session id still match.
        let mut anonymous = record("s1", Some("/repo"), &entries);
        anonymous.session_id = None;
        let none = ChangesFlowProps {
            client: None,
            session: None,
            changes: &anonymous,
            focused: false,
        };
        assert!(ChangesFlow::current(&none).is_some());
    }

    #[test]
    fn keys_move_the_selection_clamped_and_keep_it_across_refreshes() {
        local(async {
            let (app, _rx) = app(AppProps::default());
            let s1 = session("s1", "One", SessionStatus::Running);
            let entries = files(&[
                ("staged", "s.txt", "M"),
                ("unstaged", "a.txt", "M"),
                ("unstaged", "b.txt", "M"),
                ("untracked", "n.txt", "?"),
            ]);
            let changes = record("s1", Some("/repo"), &entries);
            let p = props(None, &s1, &changes, true);
            let mut flow = ChangesFlow::default();
            let mut cx = FlowCx::new(app.dispatch().clone(), |app: &mut App| &mut app.flows.changes);
            flow.sync(&p, &mut cx);
            assert_eq!(selected_path(&flow, &p).as_deref(), Some("s.txt"));
            let mut step = |flow: &mut ChangesFlow, input: Input| -> Option<String> {
                flow.handle_input(&p, &input, &mut cx);
                flow.sync(&p, &mut cx);
                selected_path(flow, &p)
            };
            assert_eq!(step(&mut flow, press("j")).as_deref(), Some("a.txt"));
            let down = named(Key {
                down_arrow: true,
                ..Key::NONE
            });
            assert_eq!(step(&mut flow, down).as_deref(), Some("b.txt"));
            assert_eq!(step(&mut flow, press("G")).as_deref(), Some("n.txt"));
            assert_eq!(step(&mut flow, press("j")).as_deref(), Some("n.txt"));
            assert_eq!(step(&mut flow, press("k")).as_deref(), Some("b.txt"));
            assert_eq!(flow.selection.offset, Some(1));
            let home = named(Key {
                home: true,
                ..Key::NONE
            });
            assert_eq!(step(&mut flow, home).as_deref(), Some("s.txt"));
            let up = named(Key {
                up_arrow: true,
                ..Key::NONE
            });
            assert_eq!(step(&mut flow, up).as_deref(), Some("s.txt"));
            let end = named(Key { end: true, ..Key::NONE });
            assert_eq!(step(&mut flow, end).as_deref(), Some("n.txt"));
            assert_eq!(step(&mut flow, press("g")).as_deref(), Some("s.txt"));
            assert_eq!(step(&mut flow, press("j")).as_deref(), Some("a.txt"));

            // After a.txt is staged (b.txt keeps its place) the selection follows the entry by group + path.
            let staged = files(&[
                ("staged", "s.txt", "M"),
                ("staged", "a.txt", "M"),
                ("unstaged", "b.txt", "M"),
                ("untracked", "n.txt", "?"),
            ]);
            let after = record("s1", Some("/repo"), &staged);
            let p2 = props(None, &s1, &after, true);
            flow.sync(&p2, &mut cx);
            assert_eq!(selected_path(&flow, &p2).as_deref(), Some("b.txt"));
            assert_eq!(flow.selection.key.as_deref(), Some("unstaged\0b.txt"));
            // The selected entry's group emptied: the first entry below it takes over.
            let rest = files(&[("staged", "s.txt", "M"), ("untracked", "n.txt", "?")]);
            let gone = record("s1", Some("/repo"), &rest);
            let p3 = props(None, &s1, &gone, true);
            flow.sync(&p3, &mut cx);
            assert_eq!(selected_path(&flow, &p3).as_deref(), Some("n.txt"));
        });
    }

    #[test]
    fn a_new_session_resets_selection_diff_and_scroll() {
        local(async {
            let (mut app, _rx) = app(AppProps::default());
            let s1 = session("s1", "One", SessionStatus::Running);
            let s2 = session("s2", "Two", SessionStatus::Running);
            let first = files(&[("unstaged", "a.txt", "M"), ("unstaged", "b.txt", "M")]);
            let second = files(&[("unstaged", "x.txt", "M"), ("unstaged", "y.txt", "M")]);
            let c1 = record("s1", Some("/repo"), &first);
            let c2 = record("s2", Some("/repo2"), &second);
            let p1 = props(None, &s1, &c1, false);
            let (_, effects) = call(&mut app, |flow, cx| {
                flow.sync(&p1, cx);
                flow.move_to(1, &p1);
                flow.sync(&p1, cx);
                flow.diff = Some(DiffState {
                    key: "unstaged\0b.txt".to_owned(),
                    ..DiffState::default()
                });
                flow.diff_scroll = 4;
            });
            assert!(effects.is_empty());
            assert_eq!(selected_path(&app.flows.changes, &p1).as_deref(), Some("b.txt"));
            let p2 = props(None, &s2, &c2, false);
            call(&mut app, |flow, cx| flow.sync(&p2, cx));
            let flow = &app.flows.changes;
            assert_eq!(selected_path(flow, &p2).as_deref(), Some("x.txt"));
            assert_eq!(flow.selection.key.as_deref(), Some("unstaged\0x.txt"));
            assert!(flow.diff.is_none());
            assert_eq!(flow.diff_scroll, 0);
        });
    }

    #[test]
    fn back_keys_exit_and_o_attaches_lazygit() {
        local(async {
            let (mut app, _rx) = app(AppProps::default());
            let changes = record("s1", Some("/repo"), &[]);
            let p = ChangesFlowProps {
                client: None,
                session: None,
                changes: &changes,
                focused: true,
            };
            let inputs = [
                named(Key {
                    escape: true,
                    ..Key::NONE
                }),
                named(Key {
                    left_arrow: true,
                    ..Key::NONE
                }),
                press("h"),
            ];
            for input in inputs {
                let (_, effects) = call(&mut app, |flow, cx| flow.handle_input(&p, &input, cx));
                assert_eq!(debug(&effects), ["Exit"]);
            }
            let (_, effects) = call(&mut app, |flow, cx| flow.handle_input(&p, &press("o"), cx));
            assert_eq!(debug(&effects), ["AttachGit"]);
            // Without a client the staging and open keys do nothing.
            let enter = named(Key {
                return_: true,
                ..Key::NONE
            });
            for input in [press(" "), press("a"), press("A"), press("E"), press("x"), enter] {
                let (_, effects) = call(&mut app, |flow, cx| flow.handle_input(&p, &input, cx));
                assert!(effects.is_empty(), "{}", input.input);
            }
        });
    }

    #[test]
    fn the_diff_scrolls_by_steps_and_pages_within_its_length() {
        let mut flow = ChangesFlow::default();
        let lines: String = (0..30).map(|n| format!("+line {n}\n")).collect();
        let diff: deckhand_protocol::data::ChangeDiff = serde_json::from_value(json!({
            "group": "unstaged", "path": "a.txt", "text": lines,
            "truncated": false, "binary": false, "firstLine": 1,
        }))
        .unwrap_or_else(|error| panic!("{error}"));
        let total = diff_lines(Some(&diff)).len();
        flow.selection.key = Some("unstaged\0a.txt".to_owned());
        flow.diff = Some(DiffState {
            key: "unstaged\0a.txt".to_owned(),
            diff: Some(diff),
            error: None,
        });
        flow.diff_rows = 10;
        flow.scroll_diff(to_isize(DIFF_STEP));
        assert_eq!(flow.diff_scroll, 3);
        flow.scroll_diff(-1);
        assert_eq!(flow.diff_scroll, 2);
        flow.scroll_diff(-10);
        assert_eq!(flow.diff_scroll, 0);
        flow.scroll_diff(1000);
        assert_eq!(flow.diff_scroll, total - 10);
        // A diff of another entry scrolls nothing.
        flow.selection.key = Some("unstaged\0b.txt".to_owned());
        flow.scroll_diff(5);
        assert_eq!(flow.diff_scroll, 0);
    }

    #[test]
    fn the_selected_diff_is_fetched_after_the_debounce_and_again_when_the_record_changes() {
        local(async {
            let (client, requests) = fake_daemon(|request| match request["type"].as_str() {
                Some("changes-diff") => Ok(diff_json(request["path"].as_str().unwrap_or(""), 7)),
                other => Err(format!("unexpected {other:?}")),
            });
            let (mut app, mut rx) = app(AppProps::default());
            let s1 = session("s1", "One", SessionStatus::Running);
            let entries = files(&[("unstaged", "a.txt", "M"), ("unstaged", "b.txt", "M")]);
            let changes = record("s1", Some("/repo"), &entries);
            let p = props(Some(&client), &s1, &changes, true);
            call(&mut app, |flow, cx| flow.sync(&p, cx));
            // Nothing is asked before the debounce passes; syncing again does not restart it.
            assert!(requests.borrow().is_empty());
            call(&mut app, |flow, cx| flow.sync(&p, cx));
            pump(&mut app, &mut rx, 2).await;
            assert_eq!(requests.borrow().len(), 1);
            let sent = requests.borrow()[0].clone();
            assert_eq!(sent["sessionId"], "s1");
            assert_eq!(sent["group"], "unstaged");
            assert_eq!(sent["path"], "a.txt");
            let diff = app.flows.changes.diff.clone().unwrap_or_else(|| panic!("no diff"));
            assert_eq!(diff.key, "unstaged\0a.txt");
            assert_eq!(diff.diff.map(|diff| diff.first_line), Some(7));

            // An equal record is not fetched again; a changed one is.
            call(&mut app, |flow, cx| flow.sync(&p, cx));
            assert_eq!(requests.borrow().len(), 1);
            let next = files(&[("unstaged", "a.txt", "M"), ("unstaged", "c.txt", "M")]);
            let refreshed = record("s1", Some("/repo"), &next);
            let p2 = props(Some(&client), &s1, &refreshed, true);
            call(&mut app, |flow, cx| flow.sync(&p2, cx));
            pump(&mut app, &mut rx, 2).await;
            assert_eq!(requests.borrow().len(), 2);

            // Moving the selection before the debounce passes asks only for the final entry.
            call(&mut app, |flow, cx| {
                flow.move_to(1, &p2);
                flow.sync(&p2, cx);
                flow.move_to(0, &p2);
                flow.sync(&p2, cx);
                flow.move_to(1, &p2);
                flow.sync(&p2, cx);
            });
            pump(&mut app, &mut rx, 2).await;
            assert_eq!(requests.borrow().len(), 3);
            assert_eq!(requests.borrow()[2]["path"], "c.txt");
        });
    }

    #[test]
    fn losing_focus_cancels_the_pending_diff_and_a_failed_fetch_shows_its_error() {
        local(async {
            let (client, requests) = fake_daemon(|_| Err("git exploded".to_owned()));
            let (mut app, mut rx) = app(AppProps::default());
            let s1 = session("s1", "One", SessionStatus::Running);
            let changes = record("s1", Some("/repo"), &files(&[("unstaged", "a.txt", "M")]));
            let focused = props(Some(&client), &s1, &changes, true);
            let blurred = props(Some(&client), &s1, &changes, false);
            call(&mut app, |flow, cx| flow.sync(&focused, cx));
            call(&mut app, |flow, cx| flow.sync(&blurred, cx));
            tokio::time::sleep(Duration::from_millis(DIFF_DEBOUNCE_MS * 3)).await;
            assert!(requests.borrow().is_empty());
            assert!(rx.try_recv().is_err());
            call(&mut app, |flow, cx| flow.sync(&focused, cx));
            pump(&mut app, &mut rx, 2).await;
            let diff = app.flows.changes.diff.clone().unwrap_or_else(|| panic!("no diff"));
            assert_eq!(diff.error.as_deref(), Some("git exploded"));
            assert!(diff.diff.is_none());
        });
    }

    #[test]
    fn staging_an_entry_reports_it_and_hands_back_the_refreshed_record() {
        local(async {
            let after = record("s1", Some("/repo"), &files(&[("staged", "a.txt", "M")]));
            let result = json!({"changed": 1, "skippedConflicts": 0, "changes": after});
            let (client, requests) = fake_daemon(move |_| Ok(result.clone()));
            let (mut app, mut rx) = app(AppProps::default());
            let s1 = session("s1", "One", SessionStatus::Running);
            let changes = record("s1", Some("/repo"), &files(&[("unstaged", "a.txt", "M")]));
            let p = props(Some(&client), &s1, &changes, false);
            call(&mut app, |flow, cx| flow.sync(&p, cx));
            let (_, effects) = call(&mut app, |flow, cx| flow.handle_input(&p, &press(" "), cx));
            assert_eq!(debug(&effects), ["SetBusy(true)", "SetError(None)"]);
            app.apply_flow_effects(effects);
            assert!(app.messages.busy);
            pump(&mut app, &mut rx, 1).await;
            assert!(!app.messages.busy);
            assert_eq!(app.messages.status_message.as_deref(), Some("Staged a.txt"));
            let sent = requests.borrow()[0].clone();
            assert_eq!(sent["type"], "change-stage");
            assert_eq!(sent["mode"], "stage");
            assert_eq!(sent["group"], "unstaged");
            assert_eq!(sent["path"], "a.txt");
        });
    }

    #[test]
    fn staging_everything_counts_files_and_mentions_skipped_conflicts() {
        local(async {
            let after = record("s1", Some("/repo"), &[]);
            let replies = Rc::new(RefCell::new(vec![
                json!({"changed": 0, "skippedConflicts": 0, "changes": after}),
                json!({"changed": 2, "skippedConflicts": 1, "changes": after}),
                json!({"changed": 1, "skippedConflicts": 2, "changes": after}),
            ]));
            let queue = Rc::clone(&replies);
            let (client, requests) = fake_daemon(move |_| Ok(queue.borrow_mut().remove(0)));
            let (mut app, mut rx) = app(AppProps::default());
            let s1 = session("s1", "One", SessionStatus::Running);
            let changes = record("s1", Some("/repo"), &files(&[("unstaged", "a.txt", "M")]));
            let p = props(Some(&client), &s1, &changes, false);
            let mut statuses = Vec::new();
            for text in ["a", "a", "A"] {
                let (_, effects) = call(&mut app, |flow, cx| flow.handle_input(&p, &press(text), cx));
                app.apply_flow_effects(effects);
                pump(&mut app, &mut rx, 1).await;
                statuses.push(app.messages.status_message.clone().unwrap_or_default());
            }
            assert_eq!(
                statuses,
                [
                    "Nothing to stage",
                    "Staged 2 files · 1 conflicted file left: stage each with space once resolved",
                    "Unstaged 1 file · 2 conflicted files left: stage each with space once resolved",
                ]
            );
            let sent = requests.borrow();
            let modes: Vec<Value> = sent.iter().map(|request| request["mode"].clone()).collect();
            assert_eq!(modes, [json!("stage"), json!("stage"), json!("unstage")]);
            assert!(
                sent.iter()
                    .all(|request| request.get("group").is_none() && request.get("path").is_none())
            );
        });
    }

    #[test]
    fn a_failed_stage_shows_the_error_and_clears_busy() {
        local(async {
            let (client, _requests) = fake_daemon(|_| Err("index.lock exists".to_owned()));
            let (mut app, mut rx) = app(AppProps::default());
            let s1 = session("s1", "One", SessionStatus::Running);
            let changes = record("s1", Some("/repo"), &files(&[("unstaged", "a.txt", "M")]));
            let p = props(Some(&client), &s1, &changes, false);
            let (_, effects) = call(&mut app, |flow, cx| flow.handle_input(&p, &press("a"), cx));
            app.apply_flow_effects(effects);
            pump(&mut app, &mut rx, 1).await;
            assert_eq!(app.messages.error.as_deref(), Some("index.lock exists"));
            assert!(!app.messages.busy);
            // No client: nothing happens.
            let offline = props(None, &s1, &changes, false);
            let (_, effects) = call(&mut app, |flow, cx| flow.handle_input(&offline, &press("a"), cx));
            assert!(effects.is_empty());
        });
    }

    #[test]
    fn open_goes_to_the_first_changed_line_and_never_for_a_deleted_file() {
        local(async {
            let workspace = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
            for name in ["a.txt", "n.txt", "z.txt"] {
                std::fs::write(workspace.path().join(name), "x\n").unwrap_or_else(|error| panic!("{error}"));
            }
            let root = workspace.path().to_str().unwrap_or_default().to_owned();
            let (client, requests) = fake_daemon(|request| match request["path"].as_str() {
                Some("z.txt") => Err("no such diff".to_owned()),
                other => Ok(diff_json(other.unwrap_or(""), 12)),
            });
            let (mut app, mut rx) = app(AppProps::default());
            let s1 = session("s1", "One", SessionStatus::Running);
            let entries = files(&[
                ("unstaged", "a.txt", "M"),
                ("unstaged", "gone.txt", "D"),
                ("unstaged", "z.txt", "M"),
                ("untracked", "n.txt", "?"),
            ]);
            let changes = record("s1", Some(&root), &entries);
            let p = props(Some(&client), &s1, &changes, false);
            let file = |name: &str| workspace.path().join(name).to_str().unwrap_or_default().to_owned();
            call(&mut app, |flow, cx| flow.sync(&p, cx));

            // Not shown yet: the diff is fetched to find the line.
            let (_, effects) = call(&mut app, |flow, cx| flow.open_with(&p, cx, fake_opener));
            assert!(effects.is_empty());
            pump(&mut app, &mut rx, 1).await;
            assert_eq!(opened(), [(file("a.txt"), Some(12))]);
            assert_eq!(app.messages.status_message.as_deref(), Some("Opened a.txt:12 in Code"));
            assert_eq!(requests.borrow().len(), 1);

            // The diff on screen is used without asking again.
            app.flows.changes.diff = Some(DiffState {
                key: "unstaged\0a.txt".to_owned(),
                diff: serde_json::from_value(diff_json("a.txt", 3)).ok(),
                error: None,
            });
            let (_, effects) = call(&mut app, |flow, cx| flow.open_with(&p, cx, fake_opener));
            assert_eq!(debug(&effects), ["SetStatus(Some(\"Opened a.txt:3 in Code\"))"]);
            assert_eq!(requests.borrow().len(), 1);

            // A deleted file cannot be opened.
            call(&mut app, |flow, _cx| flow.move_to(1, &p));
            let (_, effects) = call(&mut app, |flow, cx| flow.open_with(&p, cx, fake_opener));
            let deleted = "SetStatus(Some(\"gone.txt is deleted; nothing to open (o opens lazygit)\"))";
            assert_eq!(debug(&effects), [deleted]);

            // A diff that cannot be loaded opens at line 1.
            call(&mut app, |flow, _cx| flow.move_to(2, &p));
            let (_, effects) = call(&mut app, |flow, cx| flow.open_with(&p, cx, fake_opener));
            assert!(effects.is_empty());
            pump(&mut app, &mut rx, 1).await;
            assert_eq!(opened().last(), Some(&(file("z.txt"), Some(1))));
            assert_eq!(app.messages.status_message.as_deref(), Some("Opened z.txt:1 in Code"));

            // Untracked files have no diff to ask for.
            let before = requests.borrow().len();
            call(&mut app, |flow, _cx| flow.move_to(3, &p));
            let (_, effects) = call(&mut app, |flow, cx| flow.open_with(&p, cx, fake_opener));
            assert_eq!(debug(&effects), ["SetStatus(Some(\"Opened n.txt:1 in Code\"))"]);
            assert_eq!(requests.borrow().len(), before);

            // The editor's error goes to the footer.
            let (_, effects) = call(&mut app, |flow, cx| flow.open_with(&p, cx, failing_opener));
            let missing = "SetError(Some(\"could not find cursor or code command on PATH\"))";
            assert_eq!(debug(&effects), [missing]);
        });
    }

    #[test]
    fn rendering_records_the_diff_rows_and_draws_the_list() {
        let s1 = session("s1", "One", SessionStatus::Running);
        let changes = record("s1", Some("/repo"), &files(&[("unstaged", "a.txt", "M")]));
        let p = props(None, &s1, &changes, true);
        let mut flow = ChangesFlow::default();
        assert_eq!(flow.diff_rows, 10);
        let area = Rect::new(0, 0, 120, 30);
        let mut buf = Buffer::empty(area);
        flow.render(&p, &mut buf, area);
        assert_eq!(flow.diff_rows, changes_layout(120, 30, 2).diff_rows);
        let text: String = buf.content().iter().map(|cell| cell.symbol()).collect();
        assert!(text.contains("a.txt"), "{text}");
    }

    #[test]
    fn the_keymap_is_documented_in_the_guide() {
        assert!(unknown_topics(KEYMAP).is_empty());
        // h is the one deliberately undocumented key (its binding says why).
        assert_eq!(undocumented_in_topic(KEYMAP, "Git & PRs"), ["h"]);
    }
}
