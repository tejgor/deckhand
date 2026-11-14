//! Tasks as pure data: one Markdown file per repository, parsed into lines and changed only through TaskOps, which
//! edit the list in place and keep every other line as written. The list nests (`task_list` has the file's rules and
//! the tree): a checklist item is a line, the indented checklist items under it are its lines, and the other indented
//! text under it its details. Deckhand's bookkeeping rides at the end of the item line in a hidden comment
//! (`<!-- dh:t=3fa9c1d2 s=<session id> added=2026-10-09 -->`).
//!
//! t: the line's ID (given on Deckhand's first write of the file); s: the session on it; wt: (legacy) the worktree
//! incarnation it was for; done: when it was ticked; auto: ticked by a merge, by D, or because every line under it
//! was (`lines`), so undoing that reopens it; tried: the branch of work dropped unmerged; from: the branch it was left
//! open in when its work ended; added: when it was added. Before steps, a task could also be assigned to work as a
//! follow-up (`assigned=`, `was=`): migrate_follow_ups moves those into their work's note once.
//!
//! Times are milliseconds since the epoch (TS `Date` arguments, `new Date()` by default: callers pass
//! `jsstr::date::now_ms()`).

use std::cmp::Ordering;
use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use serde_with::skip_serializing_none;
use sha2::{Digest, Sha256};

use crate::error::{DhError, Result};
use crate::jsstr::{self, is_js_whitespace};
use crate::task_list::{self, Part, Render};
use deckhand_protocol::data::{Task, TaskAuto, TaskLink, TaskMeta, TaskOp};

/// TS: tasks.ts MAX_TASK_TITLE (UTF-16 units).
pub const MAX_TASK_TITLE: usize = 300;

/// TS: tasks.ts MAX_TASK_BODY (UTF-16 units).
pub const MAX_TASK_BODY: usize = 4000;

/// TS: tasks.ts RECENT_DONE_DAYS.
pub const RECENT_DONE_DAYS: i64 = 7;

/// TS: tasks.ts META_KEYS (the bookkeeping keys, in the order they are written).
pub const META_KEYS: [&str; 8] = ["t", "wt", "s", "done", "auto", "tried", "from", "added"];

pub(crate) static META: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[ \t]*<!--[ \t]*dh:([^>]*?)[ \t]*-->[ \t]*$").expect("META is a valid regex"));
static COMMENT_MARKER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"<!--|-->").expect("COMMENT_MARKER is a valid regex"));
// `.` in JS stops at every line terminator, not only `\n`.
static NOTE_LINK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^([ \t]*)[-*+][ \t]+↗[ \t]+([^\n\r\x{2028}\x{2029}]*?)[ \t]*<!--[ \t]*dh:t=([0-9a-z]+)[ \t]*-->[ \t]*$",
    )
    .expect("NOTE_LINK is a valid regex")
});
static TASK_META: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[ \t]*<!--[ \t]*dh:[^>]*?-->").expect("TASK_META is a valid regex"));

/// `/(?:^|[ \t])(?:<one of keys>)=\S/.test(text)` (JS `\S`: not JS whitespace).
pub(crate) fn has_bare_key(text: &str, keys: &[&str]) -> bool {
    keys.iter().any(|key| {
        let needle = format!("{key}=");
        text.match_indices(&needle).any(|(at, _)| {
            let before_ok = at == 0 || matches!(text.as_bytes().get(at - 1), Some(b' ' | b'\t'));
            let after = text.get(at + needle.len()..).and_then(|rest| rest.chars().next());
            before_ok && after.is_some_and(|ch| !is_js_whitespace(ch))
        })
    })
}

/// Read only by migrate_follow_ups (never written): a task assigned to work as a follow-up, and where a merge
/// released it from.
fn has_legacy_meta(text: &str) -> bool {
    has_bare_key(text, &["assigned", "was"])
}

/// TS: tasks.ts parseMeta (the `dh:` comment's pairs; unknown keys and empty values dropped).
pub fn parse_meta(raw: &str) -> TaskMeta {
    let mut meta = TaskMeta::default();
    for pair in raw.split([' ', '\t']) {
        let Some(at) = pair.find('=') else { continue };
        if at < 1 {
            continue;
        }
        let (key, value) = (&pair[..at], &pair[at + 1..]);
        if value.is_empty() {
            continue;
        }
        let value = value.to_owned();
        match key {
            "t" => meta.t = Some(value),
            "wt" => meta.wt = Some(value),
            "s" => meta.s = Some(value),
            "done" => meta.done = Some(value),
            "auto" => match value.as_str() {
                "merge" => meta.auto = Some(TaskAuto::Merge),
                "done" => meta.auto = Some(TaskAuto::Done),
                "lines" => meta.auto = Some(TaskAuto::Lines),
                _ => {}
            },
            "tried" => meta.tried = Some(value),
            "from" => meta.from = Some(value),
            "added" => meta.added = Some(value),
            _ => {}
        }
    }
    meta
}

/// The meta value under `key` (META_KEYS order), as written.
fn meta_value<'a>(meta: &'a TaskMeta, key: &str) -> Option<&'a str> {
    match key {
        "t" => meta.t.as_deref(),
        "wt" => meta.wt.as_deref(),
        "s" => meta.s.as_deref(),
        "done" => meta.done.as_deref(),
        "auto" => meta.auto.map(TaskAuto::as_str),
        "tried" => meta.tried.as_deref(),
        "from" => meta.from.as_deref(),
        "added" => meta.added.as_deref(),
        _ => None,
    }
}

/// TS: tasks.ts formatMeta (` <!-- dh:... -->`, or empty without keys).
pub fn format_meta(meta: &TaskMeta) -> String {
    let pairs: Vec<String> = META_KEYS
        .iter()
        .filter_map(|key| {
            let value = meta_value(meta, key).filter(|value| !value.is_empty())?;
            let value: String = value.chars().filter(|&ch| ch != '>' && !is_js_whitespace(ch)).collect();
            Some(format!("{key}={value}"))
        })
        .collect();
    if pairs.is_empty() {
        String::new()
    } else {
        format!(" <!-- dh:{} -->", pairs.join(" "))
    }
}

/// TS: tasks.ts cleanTaskTitle (one safe line: whitespace collapsed, no comment markers, cut to MAX_TASK_TITLE).
pub fn clean_task_title(title: &str) -> String {
    let mut spaced = String::with_capacity(title.len());
    let mut in_control = false;
    for ch in title.chars() {
        if matches!(ch, '\u{0}'..='\u{1F}' | '\u{7F}' | '\u{2028}' | '\u{2029}') {
            if !in_control {
                spaced.push(' ');
            }
            in_control = true;
        } else {
            spaced.push(ch);
            in_control = false;
        }
    }
    let unmarked = COMMENT_MARKER.replace_all(&spaced, "");
    let collapsed = jsstr::js_collapse_whitespace(&unmarked);
    jsstr::js_slice(jsstr::js_trim(&collapsed), 0, Some(MAX_TASK_TITLE as i64))
}

/// TS: tasks.ts cleanBody (the body's lines, without control characters, trailing spaces and blank ends).
pub(crate) fn clean_body(body: &str) -> Vec<String> {
    let normalized = body.replace("\r\n", "\n").replace('\r', "\n");
    let stripped: String = normalized
        .chars()
        .filter(|ch| !matches!(ch, '\u{0}'..='\u{8}' | '\u{B}'..='\u{1F}' | '\u{7F}'))
        .collect();
    let cut = jsstr::js_slice(&stripped, 0, Some(MAX_TASK_BODY as i64));
    let mut lines: Vec<String> = cut
        .split('\n')
        .map(|line| jsstr::js_trim_end(line).to_owned())
        .collect();
    while lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    let lead = lines.iter().take_while(|line| line.is_empty()).count();
    lines.drain(..lead);
    // A blank line would end the body when the file is read again: serialize keeps it as an indented empty line.
    lines
}

/// The leading `[ \t]*` length (bytes: ASCII).
fn indent_len(line: &str) -> usize {
    line.len() - line.trim_start_matches([' ', '\t']).len()
}

/// TS: tasks.ts dedent (the lines with their common indent removed).
pub(crate) fn dedent(lines: &[String]) -> String {
    let cut = lines
        .iter()
        .filter(|line| !jsstr::js_trim(line).is_empty())
        .map(|line| indent_len(line))
        .min()
        .unwrap_or(0);
    lines
        .iter()
        .map(|line| &line[cut.min(indent_len(line))..])
        .collect::<Vec<_>>()
        .join("\n")
}

/// `text` with `\r\n` and lone `\r` as `\n`.
pub(crate) fn normalize_newlines(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

fn short_hash(text: &str) -> String {
    let digest = Sha256::digest(text.as_bytes());
    digest.iter().take(4).map(|byte| format!("{byte:02x}")).collect()
}

pub(crate) fn provisional_id(index: usize, title: &str) -> String {
    format!("~{index}:{}", short_hash(title))
}

/// TS: tasks.ts newTaskId (8 random hex digits).
pub fn new_task_id() -> String {
    // The first bytes of a v4 UUID are all random (the version and variant bits sit later).
    let uuid = uuid::Uuid::new_v4();
    uuid.as_bytes()
        .iter()
        .take(4)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// The file's top-level lines, in file order, each with its details as `body` (the lines under them are left out:
/// `task_list::TaskList` reads the whole tree). A line without an ID yet gets a provisional one,
/// `~<position among all the file's lines>:<hash of its title>`, which ops accept until the next write gives it one.
pub fn parse_tasks(text: &str) -> Vec<Task> {
    task_list::top_level_tasks(text)
}

/// TS: tasks.ts linkMatches.
pub fn link_matches(meta: &TaskMeta, link: &TaskLink) -> bool {
    match link {
        TaskLink::Worktree { wt } => meta.wt.as_deref() == Some(wt.as_str()),
        TaskLink::Session { s } => meta.s.as_deref() == Some(s.as_str()),
    }
}

pub(crate) fn meta_linked(meta: &TaskMeta) -> bool {
    meta.wt.as_deref().is_some_and(|wt| !wt.is_empty()) || meta.s.as_deref().is_some_and(|s| !s.is_empty())
}

/// TS: tasks.ts isLinked.
pub fn is_linked(task: &Task) -> bool {
    meta_linked(&task.meta)
}

/// TS: tasks.ts linkKey (`wt:<id>` / `s:<id>`).
pub fn link_key(meta: &TaskMeta) -> Option<String> {
    match (meta.wt.as_deref(), meta.s.as_deref()) {
        (Some(wt), _) if !wt.is_empty() => Some(format!("wt:{wt}")),
        (_, Some(s)) if !s.is_empty() => Some(format!("s:{s}")),
        _ => None,
    }
}

/// TS: tasks.ts keyOfLink.
pub fn key_of_link(link: &TaskLink) -> String {
    match link {
        TaskLink::Worktree { wt } => format!("wt:{wt}"),
        TaskLink::Session { s } => format!("s:{s}"),
    }
}

/// TS: tasks.ts linkOfKey.
pub fn link_of_key(key: &str) -> Option<TaskLink> {
    if let Some(wt) = key.strip_prefix("wt:").filter(|rest| !rest.is_empty()) {
        return Some(TaskLink::Worktree { wt: wt.to_owned() });
    }
    if let Some(s) = key.strip_prefix("s:").filter(|rest| !rest.is_empty()) {
        return Some(TaskLink::Session { s: s.to_owned() });
    }
    None
}

pub(crate) fn clear_link(meta: &mut TaskMeta) {
    meta.wt = None;
    meta.s = None;
    meta.tried = None;
    meta.from = None;
}

/// `Object.assign(meta, link)`.
pub(crate) fn set_link(meta: &mut TaskMeta, link: &TaskLink) {
    match link {
        TaskLink::Worktree { wt } => meta.wt = Some(wt.clone()),
        TaskLink::Session { s } => meta.s = Some(s.clone()),
    }
}

/// `now.toISOString().slice(0, 10)`; an invalid date throws as JS does.
pub fn today(now_ms: i64) -> Result<String> {
    let iso = jsstr::date::to_iso_string(now_ms).ok_or_else(|| DhError::msg("Invalid time value"))?;
    Ok(iso.chars().take(10).collect())
}

/// JS `<` / `>=` on strings: UTF-16 code unit order.
fn js_string_cmp(a: &str, b: &str) -> Ordering {
    a.encode_utf16().cmp(b.encode_utf16())
}

/// TS: tasks.ts applyTaskOp result (`task`: the task the op added or changed).
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TaskOpResult {
    pub text: String,
    pub task: Option<Task>,
    pub changed: usize,
}

pub(crate) const NO_LONGER_LISTED: &str =
    "That task is no longer in the list (it changed outside Deckhand?); try again";

/// One change to the list, applied to `text` as it is (lines the op doesn't concern keep their place, and every line
/// that isn't a list line keeps its bytes). Every line without an ID gets one first (so every line Deckhand has
/// written can be addressed); an unknown ID throws (the file changed meanwhile). `changed` counts the lines changed;
/// `task` is the line added or changed (none for a removal or the linked-work ops).
///
/// Errors: `That task is no longer in the list (it changed outside Deckhand?); try again`, `A task needs a title`,
/// `A task with that ID already exists`, `That task is done; space reopens it`, `That line is done; reopen it first`,
/// `Another session is on that line`, `A line can't move under itself`.
///
/// - **add**: a new open line, `added=` today: under `parent` (absent: top-level), before its `at`-th line, else
///   last (at the top level without `at`: after the last open top-level line, the backlog's end, else at the end of
///   the file). `link` puts that work on it, else `from` marks where it was left open. A parent ticked by roll-up
///   reopens.
/// - **edit**: title and details (the lines under it stay). **retitle**: the title only.
/// - **toggle** ticks by hand (`done=` today, no `auto`) or reopens (forgetting when and why it was ticked);
///   **tick** / **reopen** do one of them, changing nothing when it is so already. Then the roll-up (below).
/// - **remove**: the line and everything under it (its lines and details): a line's lines are part of it, as its
///   details are. Then the roll-up of its parent.
/// - **move**: beside `target` (before it, or after it and its lines with `after`), as its sibling, so into
///   `target`'s parent; **move-to**: under `parent` (absent: top-level), placed as **add** places a line. Either way
///   with everything under it, re-indented to its new siblings' indent; into its own subtree is refused. **reorder**:
///   one place up or down among its siblings (`J` / `K`), nothing at either end. The roll-up runs on the old and the
///   new parent.
/// - **link**: that work is on it (a new start: `tried` and `from` go). **assign** (`w`): that work, or nobody, is on
///   it; refused for a done line. **session-on**: session `s` takes the line on (`tried`, `from` go); refused for a
///   done line or one someone else is on. **session-off**: session `s` leaves its line; nothing else changes.
/// - **tick-linked**: every open line that work is on ticks with `auto` (a merge or done). With `lift`, the open lines
///   directly under each (with everything under them) move up beside it, after it, marked `from` (the branch they
///   were left open in): into its parent line, or the top level. Then the roll-up.
/// - **reopen-linked**: every line that work is on, ticked with that `auto`, reopens (lines lifted out stay where
///   they went). Then the roll-up, which reopens the lines above it ticked by roll-up.
/// - **abandon-linked**: every open line that work is on is unhooked (nobody is on it, `from` goes) and marked
///   `tried` (the branch); it stays where it is.
///
/// **One line per session**: whatever puts session `s` on a line (add, link or assign with `{s}`, session-on) takes
/// it off any other line first, ticked or not, so a session is on at most one line. (A hand edit can still put it on
/// two: `TaskList::line_of_session` answers the first, and tick-linked ticks both.)
///
/// **Roll-up**: a line whose direct lines are all ticked ticks itself (`auto=lines`); a line ticked that way reopens
/// when one of its lines is open again (reopened, added, moved in). Ticked by hand, by a merge or by done, it stays
/// ticked. It runs up from the parent of what changed and stops at the first line it leaves as it was.
///
/// **Worktree links** (`wt=`) are read, kept and matched (tick-linked, reopen-linked and abandon-linked accept
/// `{wt}`), and add, link and assign still take `{wt}`, but nothing sends one: the daemon puts sessions on lines
/// (`{s}`), and the work-items migration moved every `wt=` onto a session. session-on clears it.
pub fn apply_task_op(text: &str, op: &TaskOp, now_ms: i64) -> Result<TaskOpResult> {
    apply_task_op_with(text, op, now_ms, &mut new_task_id)
}

/// `apply_task_op`, taking each new ID from `new_id` (a fixed sequence in tests).
pub fn apply_task_op_with(
    text: &str,
    op: &TaskOp,
    now_ms: i64,
    new_id: &mut dyn FnMut() -> String,
) -> Result<TaskOpResult> {
    task_list::apply(text, op, now_ms, new_id)
}

/// A JS value as `typeof x === 'object' && x` sees it: the object's fields (an array has none of the named ones).
fn object_fields(raw: &Value) -> Option<&serde_json::Map<String, Value>> {
    raw.as_object()
}

/// The ops a client may send, checked (errors `Invalid task change: <key>`, `Unknown task change`): `add` (`title`,
/// optional `parent` and `at`), `retitle`, `toggle`, `tick`, `reopen`, `remove`, `move` (`target`, `after`),
/// `move-to` (optional `parent` and `at`), `reorder` (`down`), `session-on` (`id`, `s`) and `session-off` (`s`). An
/// add's `body`, `link`, `id` and `from` are not taken (a line's details are its note, and putting sessions on lines
/// is `session-on`); `edit`, `assign`, `link` and the linked-work ops are not a client's. The daemon also checks that
/// a `session-on`'s session belongs to the repository, and handles its own `session-line` before this.
pub fn client_task_op(raw: &Value) -> Result<TaskOp> {
    let fields = object_fields(raw);
    let get = |key: &str| fields.and_then(|fields| fields.get(key));
    let invalid = |key: &str| DhError::msg(format!("Invalid task change: {key}"));
    let text = |key: &str| -> Result<String> {
        match get(key) {
            Some(Value::String(value)) => Ok(value.clone()),
            _ => Err(invalid(key)),
        }
    };
    let id = || text("id").and_then(|id| if id.is_empty() { Err(invalid("id")) } else { Ok(id) });
    // Absent or null: none; otherwise a line ID.
    let parent = || -> Result<Option<String>> {
        match get("parent") {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(parent)) if !parent.is_empty() => Ok(Some(parent.clone())),
            _ => Err(invalid("parent")),
        }
    };
    // Absent or null: at the end; otherwise a position (0-based).
    let at = || -> Result<Option<usize>> {
        match get("at") {
            None | Some(Value::Null) => Ok(None),
            Some(value) => value
                .as_u64()
                .and_then(|at| usize::try_from(at).ok())
                .map(Some)
                .ok_or_else(|| invalid("at")),
        }
    };
    let session = || text("s").and_then(|s| if s.is_empty() { Err(invalid("s")) } else { Ok(s) });
    match get("type").and_then(Value::as_str) {
        Some("add") => Ok(TaskOp::Add {
            title: text("title")?,
            body: None,
            link: None,
            id: None,
            from: None,
            parent: parent()?,
            at: at()?,
        }),
        Some("retitle") => Ok(TaskOp::Retitle {
            id: id()?,
            title: text("title")?,
        }),
        Some("toggle") => Ok(TaskOp::Toggle { id: id()? }),
        Some("tick") => Ok(TaskOp::Tick { id: id()? }),
        Some("reopen") => Ok(TaskOp::Reopen { id: id()? }),
        Some("remove") => Ok(TaskOp::Remove { id: id()? }),
        Some("move") => {
            let id = id()?;
            let target = text("target")?;
            let after = (get("after") == Some(&Value::Bool(true))).then_some(true);
            Ok(TaskOp::Move { id, target, after })
        }
        Some("move-to") => Ok(TaskOp::MoveTo {
            id: id()?,
            parent: parent()?,
            at: at()?,
        }),
        Some("reorder") => Ok(TaskOp::Reorder {
            id: id()?,
            down: get("down") == Some(&Value::Bool(true)),
        }),
        Some("session-on") => Ok(TaskOp::SessionOn {
            id: id()?,
            s: session()?,
        }),
        Some("session-off") => Ok(TaskOp::SessionOff { s: session()? }),
        _ => Err(DhError::msg("Unknown task change")),
    }
}

// ── The board ────────────────────────────────────────────────────────────────────────────────────────────────────

/// TS: tasks.ts TaskGroups.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskGroups {
    pub progress: Vec<Task>,
    pub backlog: Vec<Task>,
    pub done: Vec<Task>,
    pub older_done: Vec<Task>,
}

/// TS: tasks.ts groupTasks. Open linked tasks, open unlinked ones, then done ones: those ticked in the last
/// RECENT_DONE_DAYS (or undated, newest file position first) and the older.
pub fn group_tasks(tasks: &[Task], now_ms: i64) -> TaskGroups {
    let cutoff = today(now_ms.saturating_sub(RECENT_DONE_DAYS * 86_400_000)).unwrap_or_default();
    let done_on = |task: &Task| task.meta.done.clone().unwrap_or_default();
    let open = |linked: bool| {
        tasks
            .iter()
            .filter(|task| !task.done && is_linked(task) == linked)
            .cloned()
            .collect::<Vec<_>>()
    };
    let (progress, backlog) = (open(true), open(false));
    let mut done: Vec<Task> = tasks
        .iter()
        .filter(|task| task.done && js_string_cmp(&done_on(task), &cutoff) != Ordering::Less)
        .cloned()
        .collect();
    done.sort_by(|left, right| jsstr::locale_compare(&done_on(right), &done_on(left)));
    let older_done = tasks
        .iter()
        .filter(|task| task.done && js_string_cmp(&done_on(task), &cutoff) == Ordering::Less)
        .cloned()
        .collect();
    TaskGroups {
        progress,
        backlog,
        done,
        older_done,
    }
}

/// TS: tasks.ts openTaskCount.
pub fn open_task_count(tasks: &[Task]) -> usize {
    tasks.iter().filter(|task| !task.done).count()
}

/// TS: tasks.ts taskPrompt (the title, then the details on the same line). What a session started from a task finds
/// typed into its agent's input: one line, so every agent shows it in full in its input box.
pub fn task_prompt(task: &Task) -> String {
    // `.replace(/\s*\n\s*/g, ' ')`: every whitespace run holding a newline becomes one space.
    let body = jsstr::js_trim(&task.body);
    let mut joined = String::with_capacity(body.len());
    let mut run = String::new();
    let flush = |run: &mut String, joined: &mut String| {
        if run.contains('\n') {
            joined.push(' ');
        } else {
            joined.push_str(run);
        }
        run.clear();
    };
    for ch in body.chars() {
        if is_js_whitespace(ch) {
            run.push(ch);
        } else {
            flush(&mut run, &mut joined);
            joined.push(ch);
        }
    }
    flush(&mut run, &mut joined);
    if joined.is_empty() {
        task.title.clone()
    } else {
        format!("{}: {joined}", task.title)
    }
}

// ── Links from notes ─────────────────────────────────────────────────────────────────────────────────────────────

/// TS: tasks.ts parseNoteTaskLink result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NoteTaskLink {
    pub indent: String,
    pub title: String,
    pub id: String,
}

/// TS: tasks.ts parseNoteTaskLink (`- ↗ <title> <!-- dh:t=<id> -->`).
pub fn parse_note_task_link(line: &str) -> Option<NoteTaskLink> {
    let captures = NOTE_LINK.captures(line)?;
    let group = |index: usize| captures.get(index).map_or("", |found| found.as_str()).to_owned();
    Some(NoteTaskLink {
        indent: group(1),
        title: group(2),
        id: group(3),
    })
}

/// TS: tasks.ts LegacyFollowUp (a follow-up of the old model, taken out of the list for its work's note).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LegacyFollowUp {
    pub link: TaskLink,
    pub title: String,
    pub body: String,
}

/// TS: tasks.ts migrateFollowUps result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FollowUpMigration {
    pub text: String,
    pub moved: Vec<LegacyFollowUp>,
}

/// TS: tasks.ts migrateFollowUps (`None`: the file has no legacy bookkeeping). Follow-ups became steps: open tasks
/// assigned to work (`assigned=`) leave the list (`moved`, in file order) when `has_note` says that work still has a
/// note, else stay as backlog tasks; done ones and released ones only lose the old bookkeeping.
pub fn migrate_follow_ups(text: &str, has_note: &dyn Fn(&TaskLink) -> bool) -> Option<FollowUpMigration> {
    let tagged: Vec<&str> = text.split('\n').filter(|line| line.contains("dh:")).collect();
    if !has_legacy_meta(&tagged.join("\n")) {
        return None;
    }
    let mut moved = Vec::new();
    let mut kept = Vec::new();
    for mut part in task_list::parse_parts(text) {
        if let Part::Line(task) = &mut part
            && task.legacy_follow_up
            && !task.done
        {
            let link = match (task.meta.wt.as_deref(), task.meta.s.as_deref()) {
                (Some(wt), _) if !wt.is_empty() => Some(TaskLink::Worktree { wt: wt.to_owned() }),
                (_, Some(s)) if !s.is_empty() => Some(TaskLink::Session { s: s.to_owned() }),
                _ => None,
            };
            if let Some(link) = link {
                if has_note(&link) {
                    moved.push(LegacyFollowUp {
                        link,
                        title: task.title.clone(),
                        // Everything under it, as it was written: the note gets the item as it read before lines nested.
                        body: task.block_text(Render::TopLevel),
                    });
                    continue;
                }
                clear_link(&mut task.meta);
            }
        }
        kept.push(part);
    }
    // Serializing writes only the current keys, so the rest lose `assigned=` / `was=` too (top-level lines are the
    // only ones that had them; the lines under them are written back as they were).
    Some(FollowUpMigration {
        text: task_list::serialize(&kept, Render::TopLevel),
        moved,
    })
}

/// TS: tasks.ts appendNoteItem (`body` defaults to empty). The note with `title` (and its details indented under it)
/// added as an open checklist item at its end.
pub fn append_note_item(note_text: &str, title: &str, body: &str) -> String {
    let mut item = vec![format!("- [ ] {}", clean_task_title(title))];
    if !jsstr::js_trim(body).is_empty() {
        item.extend(body.split('\n').map(|line| {
            if line.is_empty() {
                String::new()
            } else {
                format!("  {line}")
            }
        }));
    }
    let base = jsstr::js_trim_end(note_text);
    let lead = if base.is_empty() {
        String::new()
    } else {
        format!("{base}\n")
    };
    format!("{lead}{}\n", item.join("\n"))
}

/// TS: tasks.ts withoutTaskMeta (hidden `<!-- dh:… -->` comments removed, for showing a line).
pub fn without_task_meta(line: &str) -> String {
    TASK_META.replace_all(line, "").into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use deckhand_testkit::{Case, replay, tags};
    use serde_json::json;

    fn now() -> i64 {
        jsstr::date::parse("2026-10-09T12:00:00Z").unwrap_or(0)
    }

    fn apply(text: &str, op: TaskOp) -> TaskOpResult {
        match apply_task_op(text, &op, now()) {
            Ok(result) => result,
            Err(error) => panic!("applyTaskOp failed: {error}"),
        }
    }

    fn op(value: Value) -> TaskOp {
        match serde_json::from_value(value) {
            Ok(op) => op,
            Err(error) => panic!("bad op: {error}"),
        }
    }

    fn meta(value: Value) -> TaskMeta {
        match serde_json::from_value(value) {
            Ok(meta) => meta,
            Err(error) => panic!("bad meta: {error}"),
        }
    }

    fn titled(title: &str, body: &str) -> Task {
        Task {
            id: String::new(),
            title: title.to_owned(),
            done: false,
            body: body.to_owned(),
            meta: TaskMeta::default(),
        }
    }

    fn titles(text: &str) -> Vec<String> {
        parse_tasks(text).into_iter().map(|task| task.title).collect()
    }

    fn matches(pattern: &str, text: &str) -> bool {
        Regex::new(pattern).is_ok_and(|regex| regex.is_match(text))
    }

    #[test]
    fn parses_tasks_and_keeps_other_lines() {
        let text = "# Tasks\n\nSome prose.\n- [ ] First <!-- dh:t=aaaa1111 wt=w1 added=2026-10-01 -->\n  detail one\n    deeper\n- [x] Second\n  - [ ] a line under it\nTrailing prose\n";
        let tasks = parse_tasks(text);
        let shape: Vec<(String, bool, String)> = tasks
            .iter()
            .map(|task| (task.title.clone(), task.done, task.body.clone()))
            .collect();
        // The top-level lines; a checklist item under one is a line of its own, not part of its details.
        assert_eq!(
            shape,
            vec![
                ("First".to_owned(), false, "detail one\n  deeper".to_owned()),
                ("Second".to_owned(), true, String::new()),
            ]
        );
        assert_eq!(
            tasks[0].meta,
            meta(json!({"t": "aaaa1111", "wt": "w1", "added": "2026-10-01"}))
        );
        assert_eq!(tasks[0].id, "aaaa1111");
        assert!(matches(r"^~1:[0-9a-f]{8}$", &tasks[1].id));
        // A no-op round trip through an op keeps the prose and gives every line an ID.
        let next = apply(text, op(json!({"type": "toggle", "id": "aaaa1111"}))).text;
        assert!(matches(
            r"^# Tasks\n\nSome prose\.\n- \[x\] First <!-- dh:t=aaaa1111 wt=w1 done=2026-10-09 added=2026-10-01 -->\n  detail one\n    deeper\n- \[x\] Second <!-- dh:t=[0-9a-f]{8} -->\n  - \[ \] a line under it <!-- dh:t=[0-9a-f]{8} -->\nTrailing prose\n$",
            &next
        ));
    }

    #[test]
    fn task_ops() {
        let mut text = apply(
            "",
            op(json!({"type": "add", "title": "  Write  docs\n", "body": "\nwhy:\n\n  because\n"})),
        )
        .text;
        assert!(matches(
            r"^- \[ \] Write docs <!-- dh:t=[0-9a-f]{8} added=2026-10-09 -->\n  why:\n  \n    because\n$",
            &text
        ));
        text = apply(&text, op(json!({"type": "add", "title": "Done one"}))).text;
        let done = parse_tasks(&text)[1].clone();
        text = apply(&text, op(json!({"type": "toggle", "id": done.id}))).text;
        text = apply(&text, op(json!({"type": "add", "title": "Third"}))).text;
        assert_eq!(titles(&text), ["Write docs", "Third", "Done one"]);
        assert_eq!(parse_tasks(&text)[0].body, "why:\n\n  because");
        let tasks = parse_tasks(&text);
        let (first, third) = (tasks[0].clone(), tasks[1].clone());
        text = apply(&text, op(json!({"type": "move", "id": third.id, "target": first.id}))).text;
        assert_eq!(titles(&text), ["Third", "Write docs", "Done one"]);
        text = apply(
            &text,
            op(json!({"type": "edit", "id": first.id, "title": "Write the docs", "body": ""})),
        )
        .text;
        assert_eq!(parse_tasks(&text)[1].body, "");
        text = apply(&text, op(json!({"type": "remove", "id": third.id}))).text;
        assert_eq!(titles(&text), ["Write the docs", "Done one"]);
        let error = apply_task_op(
            &text,
            &op(json!({"type": "toggle", "id": "nope"})),
            jsstr::date::now_ms(),
        );
        assert!(error.is_err_and(|error| error.to_string().contains("no longer in the list")));
        let error = apply_task_op(&text, &op(json!({"type": "add", "title": "  "})), jsstr::date::now_ms());
        assert!(error.is_err_and(|error| error.to_string().contains("needs a title")));
        // Toggling a done task reopens it and forgets when and why it was ticked.
        let reopened = apply(
            "- [x] x <!-- dh:t=b done=2026-10-01 auto=merge wt=w -->\n",
            op(json!({"type": "toggle", "id": "b"})),
        )
        .text;
        assert_eq!(reopened, "- [ ] x <!-- dh:t=b wt=w -->\n");
    }

    #[test]
    fn linked_work() {
        let mut text = "- [ ] A <!-- dh:t=a wt=w1 -->\n- [ ] B <!-- dh:t=b wt=w1 -->\n- [ ] C <!-- dh:t=c s=s1 -->\n- [x] D <!-- dh:t=d wt=w1 done=2026-10-01 -->\n".to_owned();
        let mut result = apply(
            &text,
            op(json!({"type": "tick-linked", "link": {"wt": "w1"}, "auto": "merge"})),
        );
        assert_eq!(result.changed, 2);
        text = result.text;
        let states: Vec<(bool, Option<TaskAuto>)> = parse_tasks(&text)
            .iter()
            .map(|task| (task.done, task.meta.auto))
            .collect();
        assert_eq!(
            states,
            [
                (true, Some(TaskAuto::Merge)),
                (true, Some(TaskAuto::Merge)),
                (false, None),
                (true, None)
            ]
        );
        // Only what the merge ticked reopens (D was ticked by hand).
        result = apply(
            &text,
            op(json!({"type": "reopen-linked", "link": {"wt": "w1"}, "auto": "merge"})),
        );
        assert_eq!(result.changed, 2);
        let done: Vec<bool> = parse_tasks(&result.text).iter().map(|task| task.done).collect();
        assert_eq!(done, [false, false, false, true]);
        let again = apply_task_op(
            &result.text,
            &op(json!({"type": "reopen-linked", "link": {"wt": "w1"}, "auto": "done"})),
            jsstr::date::now_ms(),
        );
        assert_eq!(again.map(|result| result.changed).ok(), Some(0));
        result = apply(
            &result.text,
            op(json!({"type": "abandon-linked", "link": {"wt": "w1"}, "tried": "fix-footer"})),
        );
        let metas: Vec<TaskMeta> = parse_tasks(&result.text).into_iter().map(|task| task.meta).collect();
        assert_eq!(
            metas,
            [
                meta(json!({"t": "a", "tried": "fix-footer"})),
                meta(json!({"t": "b", "tried": "fix-footer"})),
                meta(json!({"t": "c", "s": "s1"})),
                meta(json!({"t": "d", "wt": "w1", "done": "2026-10-01"})),
            ]
        );
        // Linking clears an old attempt.
        let linked = apply(
            &result.text,
            op(json!({"type": "link", "id": "a", "link": {"s": "s9"}})),
        );
        assert_eq!(parse_tasks(&linked.text)[0].meta, meta(json!({"t": "a", "s": "s9"})));
    }

    #[test]
    fn board_groups() {
        let tasks = parse_tasks(
            &[
                "- [ ] linked <!-- dh:t=1 wt=w -->",
                "- [ ] free <!-- dh:t=2 tried=x -->",
                "- [x] old <!-- dh:t=3 done=2026-09-01 -->",
                "- [x] new <!-- dh:t=4 done=2026-10-08 -->",
                "- [x] newer <!-- dh:t=5 done=2026-10-09 -->",
                "- [x] undated",
            ]
            .join("\n"),
        );
        let groups = group_tasks(&tasks, now());
        let names = |list: &[Task]| list.iter().map(|task| task.title.clone()).collect::<Vec<_>>();
        assert_eq!(names(&groups.progress), ["linked"]);
        assert_eq!(names(&groups.backlog), ["free"]);
        assert_eq!(names(&groups.done), ["newer", "new"]);
        assert_eq!(names(&groups.older_done), ["old", "undated"]);
    }

    #[test]
    fn note_lines() {
        let line = "  - ↗ Retry setup when trust changes <!-- dh:t=abcd1234 -->";
        assert_eq!(
            parse_note_task_link(line),
            Some(NoteTaskLink {
                indent: "  ".to_owned(),
                title: "Retry setup when trust changes".to_owned(),
                id: "abcd1234".to_owned(),
            })
        );
        assert_eq!(without_task_meta("- [ ] a <!-- dh:t=1 wt=2 -->"), "- [ ] a");
        assert_eq!(clean_task_title("a <!-- b --> c\u{7}"), "a b c");
        assert_eq!(
            task_prompt(&titled("Fix it", "\nwhy:\n  - it breaks\n")),
            "Fix it: why: - it breaks"
        );
        assert_eq!(task_prompt(&titled("Fix it", "")), "Fix it");
    }

    #[test]
    fn work_task_and_follow_up_migration() {
        let mut text = "- [ ] Elsewhere <!-- dh:t=b tried=old -->\n".to_owned();
        // Added for a work, or moved there with w: that work's task (no other bookkeeping; the old `tried` goes).
        text = apply(
            &text,
            op(json!({"type": "add", "title": "Add tests", "link": {"wt": "w1"}, "id": "c"})),
        )
        .text;
        text = apply(&text, op(json!({"type": "assign", "id": "b", "link": {"wt": "w2"}}))).text;
        let metas: Vec<TaskMeta> = parse_tasks(&text).into_iter().map(|task| task.meta).collect();
        assert_eq!(
            metas,
            [
                meta(json!({"t": "b", "wt": "w2"})),
                meta(json!({"t": "c", "wt": "w1", "added": "2026-10-09"}))
            ]
        );
        let again = apply(&text, op(json!({"type": "assign", "id": "b", "link": {"wt": "w2"}})));
        assert_eq!(again.changed, 0, "already there");
        // A merge (or done) ticks it; undoing that reopens it; deleted unmerged sends it back, tried there.
        let merged = apply(
            &text,
            op(json!({"type": "tick-linked", "link": {"wt": "w1"}, "auto": "merge"})),
        );
        assert_eq!(merged.changed, 1);
        let reopened = apply(
            &merged.text,
            op(json!({"type": "reopen-linked", "link": {"wt": "w1"}, "auto": "merge"})),
        );
        assert!(!parse_tasks(&reopened.text)[1].done);
        let abandoned = apply(
            &text,
            op(json!({"type": "abandon-linked", "link": {"wt": "w1"}, "tried": "feat/auth"})),
        );
        assert_eq!(
            parse_tasks(&abandoned.text)[1].meta,
            meta(json!({"t": "c", "tried": "feat/auth", "added": "2026-10-09"}))
        );
        // Back to the backlog with w; done tasks stay put.
        let backlog = apply(&text, op(json!({"type": "assign", "id": "c"})));
        assert_eq!(
            parse_tasks(&backlog.text)[1].meta,
            meta(json!({"t": "c", "added": "2026-10-09"}))
        );
        let error = apply_task_op(
            "- [x] d <!-- dh:t=d -->\n",
            &op(json!({"type": "assign", "id": "d", "link": {"s": "s1"}})),
            jsstr::date::now_ms(),
        );
        assert!(error.is_err_and(|error| error.to_string().contains("done")));

        // Follow-ups of the old model (`assigned=`): open ones leave the list for their work's note, unless that work
        // is gone (they stay, in the backlog); done and released ones only lose the old bookkeeping.
        let old = [
            "# Tasks",
            "- [ ] Ship it <!-- dh:t=a wt=w1 -->",
            "- [ ] Add tests <!-- dh:t=f wt=w1 assigned=2026-10-01 added=2026-10-01 -->",
            "  with fixtures",
            "- [ ] Gone work <!-- dh:t=g wt=w9 assigned=2026-10-01 -->",
            "- [x] Did it <!-- dh:t=d wt=w1 assigned=2026-10-01 done=2026-10-02 auto=merge -->",
            "- [ ] Released <!-- dh:t=r from=feat/x was=w3 -->",
        ]
        .join("\n");
        let has_w1 = |link: &TaskLink| matches!(link, TaskLink::Worktree { wt } if wt == "w1");
        let Some(migrated) = migrate_follow_ups(&old, &has_w1) else {
            panic!("expected a migration");
        };
        assert_eq!(
            migrated.moved,
            [LegacyFollowUp {
                link: TaskLink::Worktree { wt: "w1".to_owned() },
                title: "Add tests".to_owned(),
                body: "with fixtures".to_owned(),
            }]
        );
        assert_eq!(
            migrated.text,
            [
                "# Tasks",
                "- [ ] Ship it <!-- dh:t=a wt=w1 -->",
                "- [ ] Gone work <!-- dh:t=g -->",
                "- [x] Did it <!-- dh:t=d wt=w1 done=2026-10-02 auto=merge -->",
                "- [ ] Released <!-- dh:t=r from=feat/x -->",
                "",
            ]
            .join("\n")
        );
        assert_eq!(
            migrate_follow_ups(&migrated.text, &|_| true),
            None,
            "nothing left to migrate"
        );
        assert_eq!(migrate_follow_ups("- [ ] Plain <!-- dh:t=p -->\n", &|_| true), None);
        // Into its note: an open item at the end, its details indented under it.
        assert_eq!(
            append_note_item("# Plan\n- [x] Read\n\n", "Add tests", "with fixtures"),
            "# Plan\n- [x] Read\n- [ ] Add tests\n  with fixtures\n"
        );
        assert_eq!(append_note_item("", "First", ""), "- [ ] First\n");
    }

    /// A client changes lines, not links: the old model's ops and the daemon's own are refused, and an add's details,
    /// link, ID and origin are not taken.
    #[test]
    fn clients_send_line_ops_only() {
        let op = |raw: Value| client_task_op(&raw).map_err(|error| error.to_string());
        for refused in [
            json!({"type": "assign", "id": "a", "link": {"s": "s"}}),
            json!({"type": "edit", "id": "a", "title": "t", "body": "b"}),
            json!({"type": "link", "id": "a", "link": {"s": "s"}}),
            json!({"type": "tick-linked", "link": {"s": "s"}, "auto": "merge"}),
            json!({"type": "abandon-linked", "link": {"s": "s"}}),
        ] {
            assert_eq!(op(refused), Err("Unknown task change".to_owned()));
        }
        assert_eq!(
            op(json!({"type": "add", "title": "T", "body": "B", "link": {"wt": "w"}, "id": "forged", "from": "x"})),
            Ok(TaskOp::Add {
                title: "T".into(),
                body: None,
                link: None,
                id: None,
                from: None,
                parent: None,
                at: None
            })
        );
        assert_eq!(
            op(json!({"type": "add", "title": "T", "parent": "p1", "at": 2})),
            Ok(TaskOp::Add {
                title: "T".into(),
                body: None,
                link: None,
                id: None,
                from: None,
                parent: Some("p1".into()),
                at: Some(2)
            })
        );
        assert_eq!(
            op(json!({"type": "add", "title": "T", "at": -1})),
            Err("Invalid task change: at".into())
        );
        assert_eq!(
            op(json!({"type": "add", "title": "T", "parent": 5})),
            Err("Invalid task change: parent".into())
        );
        assert_eq!(
            op(json!({"type": "move-to", "id": "a", "parent": null})),
            Ok(TaskOp::MoveTo {
                id: "a".into(),
                parent: None,
                at: None
            })
        );
        assert_eq!(
            op(json!({"type": "reorder", "id": "a", "down": true})),
            Ok(TaskOp::Reorder {
                id: "a".into(),
                down: true
            })
        );
        assert_eq!(
            op(json!({"type": "retitle", "id": "a", "title": "New"})),
            Ok(TaskOp::Retitle {
                id: "a".into(),
                title: "New".into()
            })
        );
        assert_eq!(
            op(json!({"type": "tick", "id": "a"})),
            Ok(TaskOp::Tick { id: "a".into() })
        );
        assert_eq!(
            op(json!({"type": "reopen", "id": "a"})),
            Ok(TaskOp::Reopen { id: "a".into() })
        );
        assert_eq!(
            op(json!({"type": "session-on", "id": "a", "s": "s1"})),
            Ok(TaskOp::SessionOn {
                id: "a".into(),
                s: "s1".into()
            })
        );
        assert_eq!(
            op(json!({"type": "session-on", "id": "a"})),
            Err("Invalid task change: s".into())
        );
        assert_eq!(
            op(json!({"type": "session-off", "s": "s1"})),
            Ok(TaskOp::SessionOff { s: "s1".into() })
        );
        assert_eq!(
            op(json!({"type": "tick", "id": ""})),
            Err("Invalid task change: id".into())
        );
    }

    #[test]
    fn new_task_ids_are_eight_hex_digits() {
        let id = new_task_id();
        assert!(matches(r"^[0-9a-f]{8}$", &id), "{id}");
    }

    // ── Parity replays ────────────────────────────────────────────────────────────────────────────────────────────

    /// `undefined` for `None`, as TS returns it.
    fn maybe<T: Serialize>(value: Option<T>) -> Value {
        value.map_or_else(tags::undefined, |value| {
            serde_json::to_value(value).unwrap_or(Value::Null)
        })
    }

    /// A `Date` argument as epoch milliseconds.
    fn date_arg(case: &Case, index: usize) -> Result<i64, String> {
        let iso = tags::untag(case.arg(index));
        iso.as_str()
            .and_then(jsstr::date::parse)
            .ok_or_else(|| format!("arg {index}: not a date"))
    }

    /// A `Pick<Task, ...>` literal (or `{meta}`) as a whole Task.
    fn partial_task(value: &Value) -> Result<Task, String> {
        let value = tags::untag(value);
        let text = |key: &str| value.get(key).and_then(Value::as_str).unwrap_or("").to_owned();
        let meta = match value.get("meta") {
            Some(meta) => serde_json::from_value(meta.clone()).map_err(|error| error.to_string())?,
            None => TaskMeta::default(),
        };
        Ok(Task {
            id: text("id"),
            title: text("title"),
            done: value.get("done").and_then(Value::as_bool).unwrap_or(false),
            body: text("body"),
            meta,
        })
    }

    fn message(error: DhError) -> String {
        error.to_string()
    }

    #[test]
    fn append_note_item_parity() {
        replay("tasks/appendNoteItem", |case: &Case| {
            let body = if case.arg_is_undefined(2) { "" } else { case.arg_str(2)? };
            Ok::<_, String>(append_note_item(case.arg_str(0)?, case.arg_str(1)?, body))
        });
    }

    #[test]
    fn apply_task_op_parity() {
        replay("tasks/applyTaskOp", |case: &Case| {
            let op: TaskOp = case.arg_as(1)?;
            let now = if case.arg_is_undefined(2) {
                jsstr::date::now_ms()
            } else {
                date_arg(case, 2)?
            };
            // Lines under a line get IDs too now; a fixed sequence keeps the recorded results stable.
            let mut n = 0;
            let mut next = || {
                n += 1;
                format!("{n:08x}")
            };
            apply_task_op_with(case.arg_str(0)?, &op, now, &mut next).map_err(message)
        });
    }

    #[test]
    fn clean_task_title_parity() {
        replay("tasks/cleanTaskTitle", |case: &Case| {
            Ok::<_, String>(clean_task_title(case.arg_str(0)?))
        });
    }

    #[test]
    fn client_task_op_parity() {
        replay("tasks/clientTaskOp", |case: &Case| {
            let raw = tags::untag(case.arg(0));
            client_task_op(&raw).map_err(message)
        });
    }

    #[test]
    fn group_tasks_parity() {
        replay("tasks/groupTasks", |case: &Case| {
            let tasks: Vec<Task> = case.arg_as(0)?;
            Ok::<_, String>(group_tasks(&tasks, date_arg(case, 1)?))
        });
    }

    #[test]
    fn is_linked_parity() {
        replay("tasks/isLinked", |case: &Case| {
            Ok::<_, String>(is_linked(&partial_task(case.arg(0))?))
        });
    }

    #[test]
    fn key_of_link_parity() {
        replay("tasks/keyOfLink", |case: &Case| {
            let link: TaskLink = case.arg_as(0)?;
            Ok::<_, String>(key_of_link(&link))
        });
    }

    #[test]
    fn link_key_parity() {
        replay("tasks/linkKey", |case: &Case| {
            let meta: TaskMeta = case.arg_as(0)?;
            Ok::<_, String>(maybe(link_key(&meta)))
        });
    }

    #[test]
    fn link_matches_parity() {
        replay("tasks/linkMatches", |case: &Case| {
            let meta: TaskMeta = case.arg_as(0)?;
            let link: TaskLink = case.arg_as(1)?;
            Ok::<_, String>(link_matches(&meta, &link))
        });
    }

    #[test]
    fn link_of_key_parity() {
        replay("tasks/linkOfKey", |case: &Case| {
            Ok::<_, String>(maybe(link_of_key(case.arg_str(0)?)))
        });
    }

    #[test]
    fn migrate_follow_ups_parity() {
        replay("tasks/migrateFollowUps", |case: &Case| {
            let has_note: Box<dyn Fn(&TaskLink) -> bool> = match case.fn_name(1) {
                Some("hasAll") => Box::new(|_| true),
                Some("hasNone") => Box::new(|_| false),
                Some("hasW1") => Box::new(|link| matches!(link, TaskLink::Worktree { wt } if wt == "w1")),
                other => return Err(format!("unknown hasNote {other:?}")),
            };
            Ok(maybe(migrate_follow_ups(case.arg_str(0)?, &*has_note)))
        });
    }

    #[test]
    fn open_task_count_parity() {
        replay("tasks/openTaskCount", |case: &Case| {
            let tasks: Vec<Task> = case.arg_as(0)?;
            Ok::<_, String>(open_task_count(&tasks))
        });
    }

    #[test]
    fn parse_note_task_link_parity() {
        replay("tasks/parseNoteTaskLink", |case: &Case| {
            Ok::<_, String>(maybe(parse_note_task_link(case.arg_str(0)?)))
        });
    }

    #[test]
    fn parse_tasks_parity() {
        replay("tasks/parseTasks", |case: &Case| {
            Ok::<_, String>(parse_tasks(case.arg_str(0)?))
        });
    }

    #[test]
    fn task_prompt_parity() {
        replay("tasks/taskPrompt", |case: &Case| {
            Ok::<_, String>(task_prompt(&partial_task(case.arg(0))?))
        });
    }

    #[test]
    fn without_task_meta_parity() {
        replay("tasks/withoutTaskMeta", |case: &Case| {
            Ok::<_, String>(without_task_meta(case.arg_str(0)?))
        });
    }
}
