//! Port of tasks.ts.
//!
//! Tasks as pure data: one Markdown file per repository, parsed into tasks and changed only through TaskOps, which
//! edit the task blocks in place and keep every other line as written. A task is a top-level checklist item; the
//! indented lines under it are its body; Deckhand's bookkeeping rides at the end of the item line in a hidden
//! comment (`<!-- dh:t=3fa9c1d2 wt=<record id> added=2026-10-09 -->`).
//!
//! t: the task's ID (given on Deckhand's first write of the file); wt / s: the work the task is for (◆), a worktree
//! incarnation or (main checkout) session; done: when it was ticked; auto: ticked by a merge or by D, so undoing that
//! reopens it; tried: the branch of work dropped unmerged; from: the branch an item was left open in when its work
//! ended; added: when it was added. Before steps, a task could also be assigned to work as a follow-up (`assigned=`,
//! `was=`): migrate_follow_ups moves those into their work's note once.
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
use crate::notes::parse_checklist_line;
use deckhand_protocol::data::{Task, TaskAuto, TaskLink, TaskMeta, TaskOp};

/// TS: tasks.ts MAX_TASK_TITLE (UTF-16 units).
pub const MAX_TASK_TITLE: usize = 300;

/// TS: tasks.ts MAX_TASK_BODY (UTF-16 units).
pub const MAX_TASK_BODY: usize = 4000;

/// TS: tasks.ts RECENT_DONE_DAYS.
pub const RECENT_DONE_DAYS: i64 = 7;

/// TS: tasks.ts META_KEYS (the bookkeeping keys, in the order they are written).
pub const META_KEYS: [&str; 8] = ["t", "wt", "s", "done", "auto", "tried", "from", "added"];

static META: LazyLock<Regex> =
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
fn has_bare_key(text: &str, keys: &[&str]) -> bool {
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
fn clean_body(body: &str) -> Vec<String> {
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
fn dedent(lines: &[String]) -> String {
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

#[derive(Clone, Debug)]
struct TaskBlock {
    bullet: String,
    done: bool,
    title: String,
    body: Vec<String>,
    meta: TaskMeta,
    legacy_follow_up: bool,
    /// Its position among the file's tasks (as parsed; a new task gets the next one).
    serial: usize,
}

#[derive(Clone, Debug)]
enum Block {
    Raw(Vec<String>),
    Task(Box<TaskBlock>),
}

/// `text` with `\r\n` and lone `\r` as `\n`.
fn normalize_newlines(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

fn parse_blocks(text: &str) -> Vec<Block> {
    let normalized = normalize_newlines(text);
    let mut lines: Vec<&str> = normalized.split('\n').collect();
    if lines.last() == Some(&"") {
        lines.pop();
    }
    let mut blocks: Vec<Block> = Vec::new();
    let mut serial = 0;
    let mut index = 0;
    while index < lines.len() {
        let line = lines[index];
        let item = parse_checklist_line(line).filter(|item| item.indent.is_empty());
        let Some(item) = item else {
            if let Some(Block::Raw(raw)) = blocks.last_mut() {
                raw.push(line.to_owned());
            } else {
                blocks.push(Block::Raw(vec![line.to_owned()]));
            }
            index += 1;
            continue;
        };
        let captures = META.captures(&item.text);
        let (title, meta, legacy_follow_up) = match &captures {
            Some(captures) => {
                let whole = captures.get(0).map_or(item.text.len(), |found| found.start());
                let raw = captures.get(1).map_or("", |found| found.as_str());
                (&item.text[..whole], parse_meta(raw), has_bare_key(raw, &["assigned"]))
            }
            None => (item.text.as_str(), TaskMeta::default(), false),
        };
        let mut body = Vec::new();
        // The body: the indented lines right under the item (an indented blank line keeps it going).
        while index + 1 < lines.len() && lines[index + 1].starts_with([' ', '\t']) {
            index += 1;
            body.push(lines[index].to_owned());
        }
        blocks.push(Block::Task(Box::new(TaskBlock {
            bullet: item.bullet.clone(),
            done: item.checked,
            title: jsstr::js_trim(title).to_owned(),
            body,
            meta,
            legacy_follow_up,
            serial,
        })));
        serial += 1;
        index += 1;
    }
    blocks
}

fn serialize_blocks<'a>(blocks: impl IntoIterator<Item = &'a Block>) -> String {
    let mut out: Vec<String> = Vec::new();
    for block in blocks {
        match block {
            Block::Raw(lines) => out.extend(lines.iter().cloned()),
            Block::Task(task) => {
                out.push(format!(
                    "{} [{}] {}{}",
                    task.bullet,
                    if task.done { 'x' } else { ' ' },
                    task.title,
                    format_meta(&task.meta)
                ));
                out.extend(
                    task.body
                        .iter()
                        .map(|line| if line.is_empty() { "  ".to_owned() } else { line.clone() }),
                );
            }
        }
    }
    if out.is_empty() {
        String::new()
    } else {
        format!("{}\n", out.join("\n"))
    }
}

fn short_hash(text: &str) -> String {
    let digest = Sha256::digest(text.as_bytes());
    digest.iter().take(4).map(|byte| format!("{byte:02x}")).collect()
}

fn provisional_id(index: usize, title: &str) -> String {
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

fn task_of(block: &TaskBlock, index: usize) -> Task {
    Task {
        id: block
            .meta
            .t
            .clone()
            .unwrap_or_else(|| provisional_id(index, &block.title)),
        title: block.title.clone(),
        done: block.done,
        body: dedent(&block.body),
        meta: block.meta.clone(),
    }
}

/// TS: tasks.ts parseTasks (the file's tasks, in file order).
pub fn parse_tasks(text: &str) -> Vec<Task> {
    parse_blocks(text)
        .iter()
        .filter_map(|block| match block {
            Block::Task(task) => Some(task),
            Block::Raw(_) => None,
        })
        .enumerate()
        .map(|(index, task)| task_of(task, index))
        .collect()
}

/// TS: tasks.ts linkMatches.
pub fn link_matches(meta: &TaskMeta, link: &TaskLink) -> bool {
    match link {
        TaskLink::Worktree { wt } => meta.wt.as_deref() == Some(wt.as_str()),
        TaskLink::Session { s } => meta.s.as_deref() == Some(s.as_str()),
    }
}

fn meta_linked(meta: &TaskMeta) -> bool {
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

fn clear_link(meta: &mut TaskMeta) {
    meta.wt = None;
    meta.s = None;
    meta.tried = None;
    meta.from = None;
}

/// `Object.assign(meta, link)`.
fn set_link(meta: &mut TaskMeta, link: &TaskLink) {
    match link {
        TaskLink::Worktree { wt } => meta.wt = Some(wt.clone()),
        TaskLink::Session { s } => meta.s = Some(s.clone()),
    }
}

/// `now.toISOString().slice(0, 10)`; an invalid date throws as JS does.
fn today(now_ms: i64) -> Result<String> {
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

fn task_blocks_mut(blocks: &mut [Block]) -> impl Iterator<Item = &mut TaskBlock> {
    blocks.iter_mut().filter_map(|block| match block {
        Block::Task(task) => Some(task.as_mut()),
        Block::Raw(_) => None,
    })
}

fn position_of(blocks: &[Block], serial: usize) -> Option<usize> {
    blocks
        .iter()
        .position(|block| matches!(block, Block::Task(task) if task.serial == serial))
}

fn task_mut(blocks: &mut [Block], serial: usize) -> Result<&mut TaskBlock> {
    task_blocks_mut(blocks)
        .find(|task| task.serial == serial)
        .ok_or_else(|| DhError::msg(NO_LONGER_LISTED))
}

const NO_LONGER_LISTED: &str = "That task is no longer in the list (it changed outside Deckhand?); try again";

/// TS: tasks.ts applyTaskOp. Errors: `That task is no longer in the list (it changed outside Deckhand?); try
/// again`, `A task needs a title`, `A task with that ID already exists`, `That task is done; space reopens it`.
///
/// Tasks without an ID get one first (so every task Deckhand has written can be addressed); an unknown ID throws
/// (the file changed meanwhile).
pub fn apply_task_op(text: &str, op: &TaskOp, now_ms: i64) -> Result<TaskOpResult> {
    let mut blocks = parse_blocks(text);
    let mut ids: Vec<String> = Vec::new();
    for task in task_blocks_mut(&mut blocks) {
        ids.push(
            task.meta
                .t
                .clone()
                .unwrap_or_else(|| provisional_id(task.serial, &task.title)),
        );
    }
    for task in task_blocks_mut(&mut blocks) {
        if task.meta.t.is_none() {
            task.meta.t = Some(new_task_id());
        }
    }
    // The serial of the first task with that ID.
    let find = |id: &str| -> Result<usize> {
        ids.iter()
            .position(|known| known == id)
            .ok_or_else(|| DhError::msg(NO_LONGER_LISTED))
    };
    let date = today(now_ms)?;
    let mut target: Option<usize> = None;
    let mut changed = 0;
    match op {
        TaskOp::Add {
            title,
            body,
            link,
            id,
            from,
        } => {
            let title = clean_task_title(title);
            if title.is_empty() {
                return Err(DhError::msg("A task needs a title"));
            }
            if let Some(id) = id.as_deref()
                && !id.is_empty()
                && ids.iter().any(|known| known == id)
            {
                return Err(DhError::msg("A task with that ID already exists"));
            }
            let mut meta = TaskMeta {
                t: Some(id.clone().unwrap_or_else(new_task_id)),
                ..TaskMeta::default()
            };
            match link {
                Some(link) => set_link(&mut meta, link),
                None => meta.from = from.clone().filter(|from| !from.is_empty()),
            }
            meta.added = Some(date.clone());
            let serial = ids.len();
            let block = Block::Task(Box::new(TaskBlock {
                bullet: "-".to_owned(),
                done: false,
                title,
                body: clean_body(body.as_deref().unwrap_or(""))
                    .into_iter()
                    .map(|line| format!("  {line}"))
                    .collect(),
                meta,
                legacy_follow_up: false,
                serial,
            }));
            // New tasks go after the last open task (the backlog's end), else at the end of the file.
            let last_open = blocks
                .iter()
                .rposition(|block| matches!(block, Block::Task(task) if !task.done));
            let at = last_open.map_or(blocks.len(), |at| at + 1);
            blocks.insert(at, block);
            target = Some(serial);
            changed = 1;
        }
        TaskOp::Edit { id, title, body } => {
            let serial = find(id)?;
            target = Some(serial);
            let title = clean_task_title(title);
            if title.is_empty() {
                return Err(DhError::msg("A task needs a title"));
            }
            let task = task_mut(&mut blocks, serial)?;
            task.title = title;
            task.body = clean_body(body).into_iter().map(|line| format!("  {line}")).collect();
            changed = 1;
        }
        TaskOp::Toggle { id } => {
            let serial = find(id)?;
            target = Some(serial);
            let task = task_mut(&mut blocks, serial)?;
            task.done = !task.done;
            task.meta.done = task.done.then(|| date.clone());
            task.meta.auto = None;
            changed = 1;
        }
        TaskOp::Remove { id } => {
            let serial = find(id)?;
            target = Some(serial);
            if let Some(at) = position_of(&blocks, serial) {
                blocks.remove(at);
            }
            changed = 1;
        }
        TaskOp::Move {
            id,
            target: anchor_id,
            after,
        } => {
            let serial = find(id)?;
            target = Some(serial);
            let anchor = find(anchor_id)?;
            if anchor != serial
                && let Some(from) = position_of(&blocks, serial)
            {
                let block = blocks.remove(from);
                let to = position_of(&blocks, anchor).unwrap_or(blocks.len()) + usize::from(*after == Some(true));
                blocks.insert(to.min(blocks.len()), block);
                changed = 1;
            }
        }
        TaskOp::Link { id, link } => {
            let serial = find(id)?;
            target = Some(serial);
            let task = task_mut(&mut blocks, serial)?;
            clear_link(&mut task.meta);
            set_link(&mut task.meta, link);
            changed = 1;
        }
        TaskOp::Assign { id, link } => {
            let serial = find(id)?;
            target = Some(serial);
            let task = task_mut(&mut blocks, serial)?;
            if task.done {
                return Err(DhError::msg("That task is done; space reopens it"));
            }
            // Already there: nothing to change.
            let there = match link {
                Some(link) => link_matches(&task.meta, link),
                None => !meta_linked(&task.meta),
            };
            if !there {
                clear_link(&mut task.meta);
                if let Some(link) = link {
                    set_link(&mut task.meta, link);
                }
                changed = 1;
            }
        }
        TaskOp::TickLinked { link, auto } => {
            for task in task_blocks_mut(&mut blocks) {
                if task.done || !link_matches(&task.meta, link) {
                    continue;
                }
                task.done = true;
                task.meta.done = Some(date.clone());
                task.meta.auto = Some(*auto);
                changed += 1;
            }
        }
        TaskOp::ReopenLinked { link, auto } => {
            for task in task_blocks_mut(&mut blocks) {
                if !task.done || task.meta.auto != Some(*auto) || !link_matches(&task.meta, link) {
                    continue;
                }
                task.done = false;
                task.meta.done = None;
                task.meta.auto = None;
                changed += 1;
            }
        }
        TaskOp::AbandonLinked { link, tried } => {
            for task in task_blocks_mut(&mut blocks) {
                if task.done || !link_matches(&task.meta, link) {
                    continue;
                }
                clear_link(&mut task.meta);
                if let Some(tried) = tried.as_deref().filter(|tried| !tried.is_empty()) {
                    task.meta.tried = Some(tried.to_owned());
                }
                changed += 1;
            }
        }
    }
    let task = match (target, op) {
        (_, TaskOp::Remove { .. }) | (None, _) => None,
        (Some(serial), _) => blocks.iter().find_map(|block| match block {
            Block::Task(task) if task.serial == serial => Some(task_of(task, 0)),
            _ => None,
        }),
    };
    Ok(TaskOpResult {
        text: serialize_blocks(&blocks),
        task,
        changed,
    })
}

/// A JS value as `typeof x === 'object' && x` sees it: the object's fields (an array has none of the named ones).
fn object_fields(raw: &Value) -> Option<&serde_json::Map<String, Value>> {
    raw.as_object()
}

/// TS: tasks.ts clientLink.
fn client_link(raw: &Value) -> Result<TaskLink> {
    let fields = object_fields(raw);
    let get = |key: &str| fields.and_then(|fields| fields.get(key));
    match (get("wt"), get("s")) {
        (Some(Value::String(wt)), None) if !wt.is_empty() => Ok(TaskLink::Worktree { wt: wt.clone() }),
        (None, Some(Value::String(s))) if !s.is_empty() => Ok(TaskLink::Session { s: s.clone() }),
        _ => Err(DhError::msg("Invalid task change: link")),
    }
}

/// TS: tasks.ts clientTaskOp (the ops a client may send, checked). The daemon also checks an `add`/`assign` link
/// names work in this repository; starting (`link`) and the linked-work ops are the daemon's own.
pub fn client_task_op(raw: &Value) -> Result<TaskOp> {
    let fields = object_fields(raw);
    let get = |key: &str| fields.and_then(|fields| fields.get(key));
    let text = |key: &str| -> Result<String> {
        match get(key) {
            Some(Value::String(value)) => Ok(value.clone()),
            _ => Err(DhError::msg(format!("Invalid task change: {key}"))),
        }
    };
    let link = || get("link").map(client_link).transpose();
    match get("type").and_then(Value::as_str) {
        Some("add") => {
            let title = text("title")?;
            let body = get("body").and_then(Value::as_str).map(str::to_owned);
            Ok(TaskOp::Add {
                title,
                body,
                link: link()?,
                id: None,
                from: None,
            })
        }
        Some("assign") => {
            let id = text("id")?;
            Ok(TaskOp::Assign { id, link: link()? })
        }
        Some("edit") => {
            let id = text("id")?;
            let title = text("title")?;
            let body = text("body")?;
            Ok(TaskOp::Edit { id, title, body })
        }
        Some("toggle") => Ok(TaskOp::Toggle { id: text("id")? }),
        Some("remove") => Ok(TaskOp::Remove { id: text("id")? }),
        Some("move") => {
            let id = text("id")?;
            let target = text("target")?;
            let after = (get("after") == Some(&Value::Bool(true))).then_some(true);
            Ok(TaskOp::Move { id, target, after })
        }
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

/// TS: tasks.ts noteTaskLinkLine.
pub fn note_task_link_line(indent: &str, title: &str, id: &str) -> String {
    format!("{indent}- ↗ {title} <!-- dh:t={id} -->")
}

/// TS: tasks.ts promoteNoteLine result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromotedNoteLine {
    pub title: String,
    pub done: bool,
    pub text: String,
}

/// TS: tasks.ts promoteNoteLine (`line` 0-based). The checklist item on `line`, to send to Tasks: its title and the
/// note with that line replaced by a link to the new task (`id`); `None` unless the line is a checklist item.
pub fn promote_note_line(note_text: &str, line: usize, id: &str) -> Option<PromotedNoteLine> {
    let mut lines: Vec<String> = note_text.split('\n').map(str::to_owned).collect();
    let item = parse_checklist_line(lines.get(line)?)?;
    let title = clean_task_title(&item.text);
    if title.is_empty() {
        return None;
    }
    lines[line] = note_task_link_line(&item.indent, &title, id);
    Some(PromotedNoteLine {
        title,
        done: item.checked,
        text: lines.join("\n"),
    })
}

/// TS: tasks.ts promoteOpenNoteLines item.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromotedItem {
    pub id: String,
    pub title: String,
}

/// TS: tasks.ts promoteOpenNoteLines result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromotedNoteLines {
    pub text: String,
    pub items: Vec<PromotedItem>,
}

/// TS: tasks.ts promoteOpenNoteLines. Every open checklist item of a note becomes its `↗` link to a new task
/// (`new_id`); items without text are left alone. `items` are in note order.
pub fn promote_open_note_lines(note_text: &str, new_id: &mut dyn FnMut() -> String) -> PromotedNoteLines {
    let mut items = Vec::new();
    let lines: Vec<String> = note_text
        .split('\n')
        .map(|line| {
            let Some(item) = parse_checklist_line(line).filter(|item| !item.checked) else {
                return line.to_owned();
            };
            let title = clean_task_title(&item.text);
            if title.is_empty() {
                return line.to_owned();
            }
            let id = new_id();
            let linked = note_task_link_line(&item.indent, &title, &id);
            items.push(PromotedItem { id, title });
            linked
        })
        .collect();
    PromotedNoteLines {
        text: lines.join("\n"),
        items,
    }
}

/// TS: tasks.ts findNoteTaskLink (TS -1: `None`).
pub fn find_note_task_link(note_text: &str, id: &str) -> Option<usize> {
    note_text
        .split('\n')
        .position(|line| parse_note_task_link(line).is_some_and(|link| link.id == id))
}

/// TS: tasks.ts returnTaskToNote. Error: `That note no longer links to the task`. The link on `line` turned back
/// into an open checklist item: the task's title, its details indented under it.
pub fn return_task_to_note(note_text: &str, line: usize, task: &Task) -> Result<String> {
    let mut lines: Vec<String> = note_text.split('\n').map(str::to_owned).collect();
    let link = lines
        .get(line)
        .and_then(|text| parse_note_task_link(text))
        .ok_or_else(|| DhError::msg("That note no longer links to the task"))?;
    let mut replacement = vec![format!("{}- [ ] {}", link.indent, task.title)];
    if !task.body.is_empty() {
        replacement.extend(task.body.split('\n').map(|text| {
            if text.is_empty() {
                String::new()
            } else {
                format!("{}  {text}", link.indent)
            }
        }));
    }
    lines.splice(line..=line, replacement);
    Ok(lines.join("\n"))
}

/// TS: tasks.ts openNoteItems (the unchecked items, as task titles).
pub fn open_note_items(note_text: Option<&str>) -> Vec<String> {
    note_text
        .unwrap_or("")
        .split('\n')
        .filter_map(parse_checklist_line)
        .filter(|item| !item.checked)
        .map(|item| clean_task_title(&item.text))
        .filter(|title| !title.is_empty())
        .collect()
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
    for mut block in parse_blocks(text) {
        if let Block::Task(task) = &mut block
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
                        body: dedent(&task.body),
                    });
                    continue;
                }
                clear_link(&mut task.meta);
            }
        }
        kept.push(block);
    }
    // Serializing writes only the current keys, so the rest lose `assigned=` / `was=` too.
    Some(FollowUpMigration {
        text: serialize_blocks(&kept),
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
    use deckhand_parity::{Case, replay, tags};
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

    // covers: B-P5-01 tasks: top-level checklist items with their indented body; other lines are kept as written
    #[test]
    fn parses_tasks_and_keeps_other_lines() {
        let text = "# Tasks\n\nSome prose.\n- [ ] First <!-- dh:t=aaaa1111 wt=w1 added=2026-10-01 -->\n  detail one\n    deeper\n- [x] Second\n  - [ ] a sub-item stays body\nTrailing prose\n";
        let tasks = parse_tasks(text);
        let shape: Vec<(String, bool, String)> = tasks
            .iter()
            .map(|task| (task.title.clone(), task.done, task.body.clone()))
            .collect();
        assert_eq!(
            shape,
            vec![
                ("First".to_owned(), false, "detail one\n  deeper".to_owned()),
                ("Second".to_owned(), true, "- [ ] a sub-item stays body".to_owned()),
            ]
        );
        assert_eq!(
            tasks[0].meta,
            meta(json!({"t": "aaaa1111", "wt": "w1", "added": "2026-10-01"}))
        );
        assert_eq!(tasks[0].id, "aaaa1111");
        assert!(matches(r"^~1:[0-9a-f]{8}$", &tasks[1].id));
        // A no-op round trip through an op keeps the prose and gives every task an ID.
        let next = apply(text, op(json!({"type": "toggle", "id": "aaaa1111"}))).text;
        assert!(matches(
            r"^# Tasks\n\nSome prose\.\n- \[x\] First <!-- dh:t=aaaa1111 wt=w1 done=2026-10-09 added=2026-10-01 -->\n  detail one\n    deeper\n- \[x\] Second <!-- dh:t=[0-9a-f]{8} -->\n  - \[ \] a sub-item stays body\nTrailing prose\n$",
            &next
        ));
    }

    // covers: B-P5-02 task ops: add after the last open task, edit, toggle, move, remove; unknown IDs throw
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

    // covers: B-P5-03 linked work: tick on merge or done, reopen when undone, abandon back to the backlog
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

    // covers: B-P5-04 board groups: in progress, backlog, recent done (newest first) and older done
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

    // covers: B-P5-05 notes: a checklist line becomes a link to its task; open items; display without metadata
    #[test]
    fn note_lines() {
        let note = "scratch\n  - [ ] Retry setup when trust changes\nmore";
        let promoted = promote_note_line(note, 1, "abcd1234");
        assert_eq!(
            promoted,
            Some(PromotedNoteLine {
                title: "Retry setup when trust changes".to_owned(),
                done: false,
                text: "scratch\n  - ↗ Retry setup when trust changes <!-- dh:t=abcd1234 -->\nmore".to_owned(),
            })
        );
        let line = promoted.map(|promoted| promoted.text).unwrap_or_default();
        assert_eq!(
            parse_note_task_link(line.split('\n').nth(1).unwrap_or("")),
            Some(NoteTaskLink {
                indent: "  ".to_owned(),
                title: "Retry setup when trust changes".to_owned(),
                id: "abcd1234".to_owned(),
            })
        );
        assert_eq!(promote_note_line(note, 0, "x"), None);
        assert_eq!(promote_note_line(note, 9, "x"), None);
        assert_eq!(
            open_note_items(Some("- [ ] a\n- [x] b\n  * [ ] c\n- ↗ d <!-- dh:t=1 -->")),
            ["a", "c"]
        );
        assert_eq!(without_task_meta("- [ ] a <!-- dh:t=1 wt=2 -->"), "- [ ] a");
        assert_eq!(clean_task_title("a <!-- b --> c\u{7}"), "a b c");
        assert_eq!(
            task_prompt(&titled("Fix it", "\nwhy:\n  - it breaks\n")),
            "Fix it: why: - it breaks"
        );
        assert_eq!(task_prompt(&titled("Fix it", "")), "Fix it");
    }

    // covers: B-P5-06 a work's task: w or add with a link makes it the task of that work (◆), ticked by its merge or done; follow-ups migrate into notes
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

    // covers: B-P5-07 client ops: add and assign may carry a link; anything else about links is the daemon's
    #[test]
    fn client_ops() {
        let ok = |raw: Value| {
            client_task_op(&raw)
                .map(|op| serde_json::to_value(op).unwrap_or(Value::Null))
                .ok()
        };
        assert_eq!(
            ok(json!({"type": "add", "title": "x", "link": {"wt": "w1"}})),
            Some(json!({"type": "add", "title": "x", "link": {"wt": "w1"}}))
        );
        assert_eq!(
            ok(json!({"type": "assign", "id": "a", "link": {"s": "s1"}})),
            Some(json!({"type": "assign", "id": "a", "link": {"s": "s1"}}))
        );
        assert_eq!(
            ok(json!({"type": "assign", "id": "a"})),
            Some(json!({"type": "assign", "id": "a"}))
        );
        let fails =
            |raw: Value, needle: &str| client_task_op(&raw).is_err_and(|error| error.to_string().contains(needle));
        assert!(fails(
            json!({"type": "assign", "id": "a", "link": {"wt": "w", "s": "s"}}),
            "link"
        ));
        assert!(fails(json!({"type": "assign", "id": "a", "link": {"wt": ""}}), "link"));
        assert!(fails(
            json!({"type": "tick-linked", "link": {"wt": "w"}}),
            "Unknown task change"
        ));
    }

    // covers: B-P5-08 a task goes back to the note it was sent from: its ↗ line becomes the item again, details under it
    #[test]
    fn task_back_to_note() {
        let note = "scratch\n  - ↗ Retry setup <!-- dh:t=abcd1234 -->\nmore";
        assert_eq!(find_note_task_link(note, "abcd1234"), Some(1));
        assert_eq!(find_note_task_link(note, "other"), None);
        assert_eq!(
            return_task_to_note(note, 1, &titled("Retry setup when trust changes", "why:\n\nit breaks")).ok(),
            Some("scratch\n  - [ ] Retry setup when trust changes\n    why:\n\n    it breaks\nmore".to_owned())
        );
        assert_eq!(
            return_task_to_note(note, 1, &titled("Retry setup", "")).ok(),
            Some("scratch\n  - [ ] Retry setup\nmore".to_owned())
        );
        assert!(
            return_task_to_note(note, 0, &titled("x", ""))
                .is_err_and(|error| error.to_string().contains("no longer links"))
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

    /// A line index: TS reads `lines[-1]` as undefined, which no `usize` line reaches either.
    fn line_arg(case: &Case, index: usize) -> Result<usize, String> {
        let line = case.arg_i64(index)?;
        Ok(usize::try_from(line).unwrap_or(usize::MAX))
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
        replay("P5", "tasks/appendNoteItem", |case: &Case| {
            let body = if case.arg_is_undefined(2) { "" } else { case.arg_str(2)? };
            Ok::<_, String>(append_note_item(case.arg_str(0)?, case.arg_str(1)?, body))
        });
    }

    #[test]
    fn apply_task_op_parity() {
        replay("P5", "tasks/applyTaskOp", |case: &Case| {
            let op: TaskOp = case.arg_as(1)?;
            let now = if case.arg_is_undefined(2) {
                jsstr::date::now_ms()
            } else {
                date_arg(case, 2)?
            };
            apply_task_op(case.arg_str(0)?, &op, now).map_err(message)
        });
    }

    #[test]
    fn clean_task_title_parity() {
        replay("P5", "tasks/cleanTaskTitle", |case: &Case| {
            Ok::<_, String>(clean_task_title(case.arg_str(0)?))
        });
    }

    #[test]
    fn client_task_op_parity() {
        replay("P5", "tasks/clientTaskOp", |case: &Case| {
            let raw = tags::untag(case.arg(0));
            client_task_op(&raw).map_err(message)
        });
    }

    #[test]
    fn find_note_task_link_parity() {
        replay("P5", "tasks/findNoteTaskLink", |case: &Case| {
            let found = find_note_task_link(case.arg_str(0)?, case.arg_str(1)?);
            Ok::<_, String>(found.map_or(-1, |line| i64::try_from(line).unwrap_or(i64::MAX)))
        });
    }

    #[test]
    fn group_tasks_parity() {
        replay("P5", "tasks/groupTasks", |case: &Case| {
            let tasks: Vec<Task> = case.arg_as(0)?;
            Ok::<_, String>(group_tasks(&tasks, date_arg(case, 1)?))
        });
    }

    #[test]
    fn is_linked_parity() {
        replay("P5", "tasks/isLinked", |case: &Case| {
            Ok::<_, String>(is_linked(&partial_task(case.arg(0))?))
        });
    }

    #[test]
    fn key_of_link_parity() {
        replay("P5", "tasks/keyOfLink", |case: &Case| {
            let link: TaskLink = case.arg_as(0)?;
            Ok::<_, String>(key_of_link(&link))
        });
    }

    #[test]
    fn link_key_parity() {
        replay("P5", "tasks/linkKey", |case: &Case| {
            let meta: TaskMeta = case.arg_as(0)?;
            Ok::<_, String>(maybe(link_key(&meta)))
        });
    }

    #[test]
    fn link_matches_parity() {
        replay("P5", "tasks/linkMatches", |case: &Case| {
            let meta: TaskMeta = case.arg_as(0)?;
            let link: TaskLink = case.arg_as(1)?;
            Ok::<_, String>(link_matches(&meta, &link))
        });
    }

    #[test]
    fn link_of_key_parity() {
        replay("P5", "tasks/linkOfKey", |case: &Case| {
            Ok::<_, String>(maybe(link_of_key(case.arg_str(0)?)))
        });
    }

    #[test]
    fn migrate_follow_ups_parity() {
        replay("P5", "tasks/migrateFollowUps", |case: &Case| {
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
    fn note_task_link_line_parity() {
        replay("P5", "tasks/noteTaskLinkLine", |case: &Case| {
            Ok::<_, String>(note_task_link_line(
                case.arg_str(0)?,
                case.arg_str(1)?,
                case.arg_str(2)?,
            ))
        });
    }

    #[test]
    fn open_note_items_parity() {
        replay("P5", "tasks/openNoteItems", |case: &Case| {
            let text = if case.arg_is_undefined(0) {
                None
            } else {
                Some(case.arg_str(0)?)
            };
            Ok::<_, String>(open_note_items(text))
        });
    }

    #[test]
    fn open_task_count_parity() {
        replay("P5", "tasks/openTaskCount", |case: &Case| {
            let tasks: Vec<Task> = case.arg_as(0)?;
            Ok::<_, String>(open_task_count(&tasks))
        });
    }

    #[test]
    fn parse_note_task_link_parity() {
        replay("P5", "tasks/parseNoteTaskLink", |case: &Case| {
            Ok::<_, String>(maybe(parse_note_task_link(case.arg_str(0)?)))
        });
    }

    #[test]
    fn parse_tasks_parity() {
        replay("P5", "tasks/parseTasks", |case: &Case| {
            Ok::<_, String>(parse_tasks(case.arg_str(0)?))
        });
    }

    #[test]
    fn promote_note_line_parity() {
        replay("P5", "tasks/promoteNoteLine", |case: &Case| {
            let promoted = promote_note_line(case.arg_str(0)?, line_arg(case, 1)?, case.arg_str(2)?);
            Ok::<_, String>(maybe(promoted))
        });
    }

    #[test]
    fn promote_open_note_lines_parity() {
        replay("P5", "tasks/promoteOpenNoteLines", |case: &Case| {
            if case.fn_name(1) != Some("seq") {
                return Err(format!("unknown newId {:?}", case.fn_name(1)));
            }
            let mut n = 0;
            let mut next = || {
                n += 1;
                format!("id{n}")
            };
            Ok(promote_open_note_lines(case.arg_str(0)?, &mut next))
        });
    }

    #[test]
    fn return_task_to_note_parity() {
        replay("P5", "tasks/returnTaskToNote", |case: &Case| {
            let task = partial_task(case.arg(2))?;
            return_task_to_note(case.arg_str(0)?, line_arg(case, 1)?, &task).map_err(message)
        });
    }

    #[test]
    fn task_prompt_parity() {
        replay("P5", "tasks/taskPrompt", |case: &Case| {
            Ok::<_, String>(task_prompt(&partial_task(case.arg(0))?))
        });
    }

    #[test]
    fn without_task_meta_parity() {
        replay("P5", "tasks/withoutTaskMeta", |case: &Case| {
            Ok::<_, String>(without_task_meta(case.arg_str(0)?))
        });
    }
}
