//! The list as a tree: one Markdown file per repository whose checklist items nest. Every line of the list can have
//! lines under it and at most one session on it. Pure: text in, text out (`tasks::apply_task_op` is the way in).
//!
//! # The file
//!
//! - **A top-level line** is a checklist item (`- [ ] `, `* [x] `, `+ [X] `) at the start of a line. Its **block** is
//!   the run of lines right under it that start with a space or a tab (a line holding only spaces or tabs keeps the
//!   block going; an empty line or an unindented one ends it). Indented items outside a block are kept as prose.
//! - **Inside a block**, indentation decides the shape. A line's column counts a space as one and a tab as up to the
//!   next multiple of four. Each line belongs to the nearest line above it, in its block, that is indented less:
//!   an indented checklist item there is a line under it, any other text is part of its **details** (the legacy body,
//!   kept exactly as written; stage 2 moves details into line notes). A line of spaces only belongs to the line just
//!   above it. Any consistent indent works; Deckhand writes two spaces per level, and a line added under one with
//!   lines already under it takes their indent.
//! - **Bookkeeping** rides at the end of a line in `<!-- dh:… -->`: `t` the line's ID (every line gets one on
//!   Deckhand's first write), `s` the session on it, `wt` a worktree (legacy: read and kept, never written by the
//!   list's own ops, see `tasks::apply_task_op`), `done` / `auto` when and why it was ticked (`merge`, `done`, or
//!   `lines` for a roll-up), `tried` the branch of an attempt dropped unmerged, `from` the branch it was left open in,
//!   `added`.
//! - Every other line of the file (headings, prose, blank lines, indented prose outside a block) is kept as written,
//!   and so are a line's details. A change rewrites the list's checklist lines in Deckhand's form (as it always has:
//!   `- [x] Title <!-- dh:… -->`, keeping each line's indent and bullet). A line that moves takes its lines and details
//!   with it, shifted by the same number of columns.
//!
//! The tree can't have cycles: a line's parent is always above it in the file, and a move into a line's own subtree
//! is refused. When the same ID appears twice (a line copied in an editor), the first one is the one ops and queries
//! address, as before.

use deckhand_protocol::data::{Task, TaskAuto, TaskLink, TaskMeta, TaskOp};

use crate::error::{DhError, Result};
use crate::jsstr;
use crate::notes::parse_checklist_line;
use crate::tasks::{
    META, NO_LONGER_LISTED, TaskOpResult, clean_body, clean_task_title, clear_link, dedent, format_meta, has_bare_key,
    link_matches, meta_linked, normalize_newlines, parse_meta, provisional_id, set_link, today,
};

/// One indent level, as Deckhand writes it.
const STEP: &str = "  ";

/// A tab advances to the next multiple of this column.
const TAB_STOP: usize = 4;

/// One line of the file: prose (or, inside a line, a line of its details), or a list line with what is under it.
#[derive(Clone, Debug)]
pub(crate) enum Part {
    Text(String),
    Line(Box<Node>),
}

/// A list line and its content (its details and the lines under it, in file order).
#[derive(Clone, Debug)]
pub(crate) struct Node {
    pub(crate) indent: String,
    pub(crate) bullet: String,
    pub(crate) done: bool,
    pub(crate) title: String,
    pub(crate) meta: TaskMeta,
    /// `assigned=` on a top-level line (read only by `tasks::migrate_follow_ups`).
    pub(crate) legacy_follow_up: bool,
    /// The line as written (only the follow-up migration writes it back unchanged).
    raw: String,
    /// Its position among the file's lines as parsed (file order); a new line gets the next one.
    pub(crate) serial: usize,
    pub(crate) content: Vec<Part>,
}

impl Node {
    fn col(&self) -> usize {
        indent_col(&self.indent)
    }

    fn lines(&self) -> impl Iterator<Item = &Node> {
        self.content.iter().filter_map(|part| match part {
            Part::Line(node) => Some(node.as_ref()),
            Part::Text(_) => None,
        })
    }

    /// Its details: its own text lines, with their common indent removed.
    pub(crate) fn body(&self) -> String {
        let lines: Vec<String> = self
            .content
            .iter()
            .filter_map(|part| match part {
                Part::Text(text) => Some(text.clone()),
                Part::Line(_) => None,
            })
            .collect();
        dedent(&lines)
    }

    /// Everything under it as written (lines in Deckhand's form when `render` says so), with the common indent removed:
    /// what a top-level line's body was before lines nested.
    pub(crate) fn block_text(&self, render: Render) -> String {
        let mut out = Vec::new();
        write_parts(&self.content, render, 1, &mut out);
        dedent(&out)
    }

    fn task(&self) -> Task {
        Task {
            id: self
                .meta
                .t
                .clone()
                .unwrap_or_else(|| provisional_id(self.serial, &self.title)),
            title: self.title.clone(),
            done: self.done,
            body: self.body(),
            meta: self.meta.clone(),
        }
    }
}

/// The column a line's leading spaces and tabs reach.
fn indent_col(line: &str) -> usize {
    let mut col = 0;
    for ch in line.chars() {
        match ch {
            ' ' => col += 1,
            '\t' => col = (col / TAB_STOP + 1) * TAB_STOP,
            _ => break,
        }
    }
    col
}

fn indent_len(line: &str) -> usize {
    line.len() - line.trim_start_matches([' ', '\t']).len()
}

fn is_blank(line: &str) -> bool {
    line.trim_matches([' ', '\t']).is_empty()
}

fn spaces(col: usize) -> String {
    " ".repeat(col)
}

fn item_of(line: &str, serial: usize) -> Option<Node> {
    let item = parse_checklist_line(line)?;
    let captures = META.captures(&item.text);
    let (title, meta, legacy_follow_up) = match &captures {
        Some(captures) => {
            let whole = captures.get(0).map_or(item.text.len(), |found| found.start());
            let raw = captures.get(1).map_or("", |found| found.as_str());
            (&item.text[..whole], parse_meta(raw), has_bare_key(raw, &["assigned"]))
        }
        None => (item.text.as_str(), TaskMeta::default(), false),
    };
    Some(Node {
        legacy_follow_up: legacy_follow_up && item.indent.is_empty(),
        indent: item.indent,
        bullet: item.bullet,
        done: item.checked,
        title: jsstr::js_trim(title).to_owned(),
        meta,
        raw: line.to_owned(),
        serial,
        content: Vec::new(),
    })
}

/// Closes the open lines (innermost first) while `close` says so, keeping the block's top-level line.
fn close_while(open: &mut Vec<Node>, close: impl Fn(&Node) -> bool) {
    while open.len() > 1 && open.last().is_some_and(&close) {
        if let Some(node) = open.pop()
            && let Some(parent) = open.last_mut()
        {
            parent.content.push(Part::Line(Box::new(node)));
        }
    }
}

fn close_block(open: &mut Vec<Node>, parts: &mut Vec<Part>) {
    while let Some(node) = open.pop() {
        match open.last_mut() {
            Some(parent) => parent.content.push(Part::Line(Box::new(node))),
            None => parts.push(Part::Line(Box::new(node))),
        }
    }
}

/// The file as parts (the rules are in the module docs).
pub(crate) fn parse_parts(text: &str) -> Vec<Part> {
    let normalized = normalize_newlines(text);
    let mut lines: Vec<&str> = normalized.split('\n').collect();
    if lines.last() == Some(&"") {
        lines.pop();
    }
    let mut parts = Vec::new();
    // The open lines of the current block, its top-level line first.
    let mut open: Vec<Node> = Vec::new();
    let mut serial = 0;
    for line in lines {
        if !open.is_empty() && line.starts_with([' ', '\t']) {
            if !is_blank(line) {
                let col = indent_col(line);
                close_while(&mut open, |node| node.col() >= col);
                if let Some(node) = item_of(line, serial) {
                    serial += 1;
                    open.push(node);
                    continue;
                }
            }
            if let Some(owner) = open.last_mut() {
                owner.content.push(Part::Text(line.to_owned()));
            }
            continue;
        }
        close_block(&mut open, &mut parts);
        match item_of(line, serial).filter(|node| node.indent.is_empty()) {
            Some(node) => {
                serial += 1;
                open.push(node);
            }
            None => parts.push(Part::Text(line.to_owned())),
        }
    }
    close_block(&mut open, &mut parts);
    parts
}

/// Which list lines a write puts in Deckhand's form.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Render {
    /// Every list line (what every change does).
    All,
    /// Top-level lines only; the lines under them as written.
    TopLevel,
}

fn render_line(node: &Node) -> String {
    format!(
        "{}{} [{}] {}{}",
        node.indent,
        node.bullet,
        if node.done { 'x' } else { ' ' },
        node.title,
        format_meta(&node.meta)
    )
}

fn write_parts(parts: &[Part], render: Render, depth: usize, out: &mut Vec<String>) {
    for part in parts {
        match part {
            Part::Text(text) => out.push(text.clone()),
            Part::Line(node) => {
                out.push(if render == Render::All || depth == 0 {
                    render_line(node)
                } else {
                    node.raw.clone()
                });
                write_parts(&node.content, render, depth + 1, out);
            }
        }
    }
}

pub(crate) fn serialize(parts: &[Part], render: Render) -> String {
    let mut out = Vec::new();
    write_parts(parts, render, 0, &mut out);
    if out.is_empty() {
        String::new()
    } else {
        format!("{}\n", out.join("\n"))
    }
}

/// A list line in file order, with its parent (an index into the same order) and depth (0: top-level).
struct Flat<'a> {
    node: &'a Node,
    parent: Option<usize>,
    depth: usize,
}

fn flatten(parts: &[Part]) -> Vec<Flat<'_>> {
    fn visit<'a>(parts: &'a [Part], parent: Option<usize>, depth: usize, out: &mut Vec<Flat<'a>>) {
        for part in parts {
            if let Part::Line(node) = part {
                let at = out.len();
                out.push(Flat { node, parent, depth });
                visit(&node.content, Some(at), depth + 1, out);
            }
        }
    }
    let mut out = Vec::new();
    visit(parts, None, 0, &mut out);
    out
}

fn for_each_node_mut(parts: &mut [Part], visit: &mut dyn FnMut(&mut Node)) {
    for part in parts {
        if let Part::Line(node) = part {
            visit(node);
            for_each_node_mut(&mut node.content, visit);
        }
    }
}

/// Where the line is: the index of each part on the way down.
fn path_of(parts: &[Part], serial: usize) -> Option<Vec<usize>> {
    for (index, part) in parts.iter().enumerate() {
        if let Part::Line(node) = part {
            if node.serial == serial {
                return Some(vec![index]);
            }
            if let Some(mut rest) = path_of(&node.content, serial) {
                rest.insert(0, index);
                return Some(rest);
            }
        }
    }
    None
}

fn node_at<'a>(parts: &'a [Part], path: &[usize]) -> Option<&'a Node> {
    let (first, rest) = path.split_first()?;
    let Some(Part::Line(node)) = parts.get(*first) else {
        return None;
    };
    if rest.is_empty() {
        Some(node)
    } else {
        node_at(&node.content, rest)
    }
}

fn node_at_mut<'a>(parts: &'a mut [Part], path: &[usize]) -> Option<&'a mut Node> {
    let (first, rest) = path.split_first()?;
    let Some(Part::Line(node)) = parts.get_mut(*first) else {
        return None;
    };
    if rest.is_empty() {
        Some(node)
    } else {
        node_at_mut(&mut node.content, rest)
    }
}

/// The parts the line at `owner` holds (the file's top level for an empty path).
fn container_mut<'a>(parts: &'a mut Vec<Part>, owner: &[usize]) -> Option<&'a mut Vec<Part>> {
    if owner.is_empty() {
        Some(parts)
    } else {
        node_at_mut(parts, owner).map(|node| &mut node.content)
    }
}

/// `node` at `indent`, everything under it shifted by as many columns.
fn reindent(node: &mut Node, indent: String) {
    let delta = indent_col(&indent) as isize - node.col() as isize;
    node.indent = indent;
    if delta != 0 {
        shift_content(&mut node.content, delta);
    }
}

fn shifted(col: usize, delta: isize) -> usize {
    // Never to column 0: an unindented line would end the block.
    (col as isize + delta).max(1) as usize
}

fn shift_content(content: &mut [Part], delta: isize) {
    for part in content {
        match part {
            Part::Text(text) => *text = shift_text(text, delta),
            Part::Line(node) => {
                node.indent = spaces(shifted(node.col(), delta));
                shift_content(&mut node.content, delta);
            }
        }
    }
}

fn shift_text(text: &str, delta: isize) -> String {
    let lead = indent_len(text);
    format!("{}{}", spaces(shifted(indent_col(text), delta)), &text[lead..])
}

/// Keeps a line's content reading back as the same tree after parts were taken out or put in: a line that would
/// land under the line before it (indented more, nothing of its parent's in between) is brought out to that line's
/// indent, and so is a detail line that would land in the line before it.
fn normalise(content: &mut [Part]) {
    // The indent of the last line in `content` while nothing of the owner's has followed it.
    let mut open: Option<usize> = None;
    for part in content.iter_mut() {
        match part {
            Part::Line(node) => {
                if let Some(col) = open
                    && node.col() > col
                {
                    reindent(node, spaces(col));
                }
                open = Some(node.col());
            }
            Part::Text(text) if !is_blank(text) => {
                if let Some(col) = open {
                    let own = indent_col(text);
                    if own > col {
                        *text = shift_text(text, col as isize - own as isize);
                    }
                }
                open = None;
            }
            Part::Text(_) => {}
        }
    }
}

/// Where a line goes.
enum Spot {
    /// Under `parent` (top-level: `None`), before its `at`-th line; without `at` (or past the end), last under the
    /// parent, or at the backlog's end at the top level (after the last open top-level line, else at the end of the
    /// file).
    Under { parent: Option<usize>, at: Option<usize> },
    /// Beside `target`, before it or after it (and everything under it).
    Beside { target: usize, after: bool },
}

/// The list being changed by one op.
struct Change<'a> {
    parts: Vec<Part>,
    date: String,
    next_serial: usize,
    new_id: &'a mut dyn FnMut() -> String,
    changed: usize,
}

impl Change<'_> {
    fn path(&self, serial: usize) -> Result<Vec<usize>> {
        path_of(&self.parts, serial).ok_or_else(|| DhError::msg(NO_LONGER_LISTED))
    }

    fn node(&self, serial: usize) -> Result<&Node> {
        let path = self.path(serial)?;
        node_at(&self.parts, &path).ok_or_else(|| DhError::msg(NO_LONGER_LISTED))
    }

    fn node_mut(&mut self, serial: usize) -> Result<&mut Node> {
        let path = self.path(serial)?;
        node_at_mut(&mut self.parts, &path).ok_or_else(|| DhError::msg(NO_LONGER_LISTED))
    }

    fn parent_of(&self, serial: usize) -> Option<usize> {
        let path = path_of(&self.parts, serial)?;
        let owner = path.split_last()?.1;
        node_at(&self.parts, owner).map(|node| node.serial)
    }

    /// Whether `serial` is `root` or under it.
    fn within(&self, serial: usize, root: usize) -> bool {
        let mut current = Some(serial);
        while let Some(at) = current {
            if at == root {
                return true;
            }
            current = self.parent_of(at);
        }
        false
    }

    fn serials(&self, keep: impl Fn(&Node) -> bool) -> Vec<usize> {
        flatten(&self.parts)
            .iter()
            .filter(|flat| keep(flat.node))
            .map(|flat| flat.node.serial)
            .collect()
    }

    /// Takes the line (and everything under it) out; answers it and its parent.
    fn take(&mut self, serial: usize) -> Result<(Node, Option<usize>)> {
        let parent = self.parent_of(serial);
        let path = self.path(serial)?;
        let Some((last, owner)) = path.split_last() else {
            return Err(DhError::msg(NO_LONGER_LISTED));
        };
        let container = container_mut(&mut self.parts, owner).ok_or_else(|| DhError::msg(NO_LONGER_LISTED))?;
        let Part::Line(node) = container.remove(*last) else {
            return Err(DhError::msg(NO_LONGER_LISTED));
        };
        if !owner.is_empty() {
            normalise(container);
        }
        Ok((*node, parent))
    }

    /// Puts `node` (and everything under it) at `spot`, at the indent of its new siblings.
    fn place(&mut self, mut node: Node, spot: Spot) -> Result<()> {
        let (owner, index, indent) = match spot {
            Spot::Beside { target, after } => {
                let path = self.path(target)?;
                let Some((last, owner)) = path.split_last() else {
                    return Err(DhError::msg(NO_LONGER_LISTED));
                };
                let indent = self.node(target)?.indent.clone();
                (owner.to_vec(), last + usize::from(after), indent)
            }
            Spot::Under { parent: None, at } => {
                let lines: Vec<usize> = line_positions(&self.parts);
                let index = match at.and_then(|at| lines.get(at)) {
                    Some(&index) => index,
                    None if at.is_some() => lines.last().map_or(self.parts.len(), |last| last + 1),
                    None => self
                        .parts
                        .iter()
                        .rposition(|part| matches!(part, Part::Line(node) if !node.done))
                        .map_or(self.parts.len(), |last| last + 1),
                };
                (Vec::new(), index, String::new())
            }
            Spot::Under {
                parent: Some(parent),
                at,
            } => {
                let path = self.path(parent)?;
                let owner = self.node(parent)?;
                let lines = line_positions(&owner.content);
                let line_indent = |index: usize| match owner.content.get(index) {
                    Some(Part::Line(node)) => Some(node.indent.clone()),
                    _ => None,
                };
                let (index, indent) = match at.and_then(|at| lines.get(at)) {
                    Some(&index) => (index, line_indent(index)),
                    None => (owner.content.len(), lines.last().and_then(|&last| line_indent(last))),
                };
                let indent = indent.unwrap_or_else(|| format!("{}{STEP}", owner.indent));
                (path, index, indent)
            }
        };
        reindent(&mut node, indent);
        let container = container_mut(&mut self.parts, &owner).ok_or_else(|| DhError::msg(NO_LONGER_LISTED))?;
        container.insert(index.min(container.len()), Part::Line(Box::new(node)));
        if !owner.is_empty() {
            normalise(container);
        }
        Ok(())
    }

    fn tick(&mut self, serial: usize, auto: Option<TaskAuto>) -> Result<()> {
        let date = self.date.clone();
        let node = self.node_mut(serial)?;
        node.done = true;
        node.meta.done = Some(date);
        node.meta.auto = auto;
        self.changed += 1;
        Ok(())
    }

    fn reopen(&mut self, serial: usize) -> Result<()> {
        let node = self.node_mut(serial)?;
        node.done = false;
        node.meta.done = None;
        node.meta.auto = None;
        self.changed += 1;
        Ok(())
    }

    /// Roll-up, from `from` up: a line whose lines are all ticked ticks itself (`auto=lines`), and a line ticked that
    /// way with an open line under it reopens. Stops at the first line it leaves as it was.
    fn settle(&mut self, from: Option<usize>) -> Result<()> {
        let mut current = from;
        while let Some(serial) = current {
            let node = self.node(serial)?;
            let states: Vec<bool> = node.lines().map(|line| line.done).collect();
            if !node.done && !states.is_empty() && states.iter().all(|done| *done) {
                self.tick(serial, Some(TaskAuto::Lines))?;
            } else if node.done && node.meta.auto == Some(TaskAuto::Lines) && states.iter().any(|done| !done) {
                self.reopen(serial)?;
            } else {
                break;
            }
            current = self.parent_of(serial);
        }
        Ok(())
    }

    /// Takes session `s` off every line but `keep`; answers how many it left.
    fn unseat(&mut self, s: &str, keep: Option<usize>) -> usize {
        let mut count = 0;
        for_each_node_mut(&mut self.parts, &mut |node| {
            if Some(node.serial) != keep && node.meta.s.as_deref() == Some(s) {
                node.meta.s = None;
                count += 1;
            }
        });
        self.changed += count;
        count
    }

    /// Moves the line `serial` to `spot` with what is under it, then settles both parents.
    fn relocate(&mut self, serial: usize, spot: Spot) -> Result<()> {
        let (node, old_parent) = self.take(serial)?;
        self.place(node, spot)?;
        self.changed += 1;
        self.settle(old_parent)?;
        self.settle(self.parent_of(serial))
    }

    /// The details as lines under the line, at its indent plus one level.
    fn set_body(&mut self, serial: usize, body: &str) -> Result<()> {
        let node = self.node_mut(serial)?;
        let indent = format!("{}{STEP}", node.indent);
        let mut content: Vec<Part> = clean_body(body)
            .into_iter()
            .map(|line| Part::Text(format!("{indent}{line}")))
            .collect();
        content.extend(
            std::mem::take(&mut node.content)
                .into_iter()
                .filter(|part| matches!(part, Part::Line(_))),
        );
        normalise(&mut content);
        node.content = content;
        Ok(())
    }
}

/// The indexes of `parts` that are lines.
fn line_positions(parts: &[Part]) -> Vec<usize> {
    parts
        .iter()
        .enumerate()
        .filter(|(_, part)| matches!(part, Part::Line(_)))
        .map(|(index, _)| index)
        .collect()
}

/// A line linked to that work (legacy worktree links included).
fn linked_to(node: &Node, link: &TaskLink) -> bool {
    link_matches(&node.meta, link)
}

fn session_of(link: &TaskLink) -> Option<&str> {
    match link {
        TaskLink::Session { s } => Some(s),
        TaskLink::Worktree { .. } => None,
    }
}

/// See `tasks::apply_task_op`.
pub(crate) fn apply(text: &str, op: &TaskOp, now_ms: i64, new_id: &mut dyn FnMut() -> String) -> Result<TaskOpResult> {
    let mut parts = parse_parts(text);
    let ids: Vec<String> = flatten(&parts)
        .iter()
        .map(|flat| {
            flat.node
                .meta
                .t
                .clone()
                .unwrap_or_else(|| provisional_id(flat.node.serial, &flat.node.title))
        })
        .collect();
    for_each_node_mut(&mut parts, &mut |node| {
        if node.meta.t.is_none() {
            node.meta.t = Some(new_id());
        }
    });
    // The serial of the first line with that ID (serials are file order, as `ids` is).
    let find = |id: &str| -> Result<usize> {
        ids.iter()
            .position(|known| known == id)
            .ok_or_else(|| DhError::msg(NO_LONGER_LISTED))
    };
    let mut change = Change {
        parts,
        date: today(now_ms)?,
        next_serial: ids.len(),
        new_id,
        changed: 0,
    };
    let mut target: Option<usize> = None;
    match op {
        TaskOp::Add {
            title,
            body,
            link,
            id,
            from,
            parent,
            at,
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
            let parent = parent.as_deref().map(find).transpose()?;
            let mut meta = TaskMeta {
                t: Some(id.clone().unwrap_or_else(|| (change.new_id)())),
                ..TaskMeta::default()
            };
            match link {
                Some(link) => set_link(&mut meta, link),
                None => meta.from = from.clone().filter(|from| !from.is_empty()),
            }
            meta.added = Some(change.date.clone());
            if let Some(s) = link.as_ref().and_then(session_of) {
                change.unseat(s, None);
            }
            let serial = change.next_serial;
            change.next_serial += 1;
            let node = Node {
                indent: String::new(),
                bullet: "-".to_owned(),
                done: false,
                title,
                meta,
                legacy_follow_up: false,
                raw: String::new(),
                serial,
                content: clean_body(body.as_deref().unwrap_or(""))
                    .into_iter()
                    .map(|line| Part::Text(format!("{STEP}{line}")))
                    .collect(),
            };
            change.place(node, Spot::Under { parent, at: *at })?;
            change.changed += 1;
            change.settle(parent)?;
            target = Some(serial);
        }
        TaskOp::Edit { id, title, body } => {
            let serial = find(id)?;
            target = Some(serial);
            let title = clean_task_title(title);
            if title.is_empty() {
                return Err(DhError::msg("A task needs a title"));
            }
            change.node_mut(serial)?.title = title;
            change.set_body(serial, body)?;
            change.changed += 1;
        }
        TaskOp::Retitle { id, title } => {
            let serial = find(id)?;
            target = Some(serial);
            let title = clean_task_title(title);
            if title.is_empty() {
                return Err(DhError::msg("A task needs a title"));
            }
            change.node_mut(serial)?.title = title;
            change.changed += 1;
        }
        TaskOp::Toggle { id } | TaskOp::Tick { id } | TaskOp::Reopen { id } => {
            let serial = find(id)?;
            target = Some(serial);
            let done = change.node(serial)?.done;
            let flip = match op {
                TaskOp::Tick { .. } => !done,
                TaskOp::Reopen { .. } => done,
                _ => true,
            };
            if flip {
                if done {
                    change.reopen(serial)?;
                } else {
                    change.tick(serial, None)?;
                }
                change.settle(change.parent_of(serial))?;
            }
        }
        TaskOp::Remove { id } => {
            let serial = find(id)?;
            target = Some(serial);
            let (_, parent) = change.take(serial)?;
            change.changed += 1;
            change.settle(parent)?;
        }
        TaskOp::Move {
            id,
            target: anchor,
            after,
        } => {
            let serial = find(id)?;
            target = Some(serial);
            let anchor = find(anchor)?;
            if anchor != serial {
                if change.within(anchor, serial) {
                    return Err(DhError::msg("A line can't move under itself"));
                }
                let after = *after == Some(true);
                change.relocate(serial, Spot::Beside { target: anchor, after })?;
            }
        }
        TaskOp::MoveTo { id, parent, at } => {
            let serial = find(id)?;
            target = Some(serial);
            let parent = parent.as_deref().map(find).transpose()?;
            if parent.is_some_and(|parent| change.within(parent, serial)) {
                return Err(DhError::msg("A line can't move under itself"));
            }
            change.relocate(serial, Spot::Under { parent, at: *at })?;
        }
        TaskOp::Reorder { id, down } => {
            let serial = find(id)?;
            target = Some(serial);
            let siblings: Vec<usize> = match change.parent_of(serial) {
                Some(parent) => change.node(parent)?.lines().map(|line| line.serial).collect(),
                None => change
                    .parts
                    .iter()
                    .filter_map(|part| match part {
                        Part::Line(node) => Some(node.serial),
                        Part::Text(_) => None,
                    })
                    .collect(),
            };
            let at = siblings.iter().position(|sibling| *sibling == serial);
            let neighbour = match (at, down) {
                (Some(at), true) => siblings.get(at + 1),
                (Some(at), false) => at.checked_sub(1).and_then(|at| siblings.get(at)),
                (None, _) => None,
            };
            if let Some(&neighbour) = neighbour {
                let after = *down;
                change.relocate(
                    serial,
                    Spot::Beside {
                        target: neighbour,
                        after,
                    },
                )?;
            }
        }
        TaskOp::Link { id, link } => {
            let serial = find(id)?;
            target = Some(serial);
            if let Some(s) = session_of(link) {
                change.unseat(s, Some(serial));
            }
            let node = change.node_mut(serial)?;
            clear_link(&mut node.meta);
            set_link(&mut node.meta, link);
            change.changed += 1;
        }
        TaskOp::Assign { id, link } => {
            let serial = find(id)?;
            target = Some(serial);
            let node = change.node(serial)?;
            if node.done {
                return Err(DhError::msg("That task is done; space reopens it"));
            }
            // Already there: nothing to change.
            let there = match link {
                Some(link) => link_matches(&node.meta, link),
                None => !meta_linked(&node.meta),
            };
            if !there {
                if let Some(s) = link.as_ref().and_then(session_of) {
                    change.unseat(s, Some(serial));
                }
                let node = change.node_mut(serial)?;
                clear_link(&mut node.meta);
                if let Some(link) = link {
                    set_link(&mut node.meta, link);
                }
                change.changed += 1;
            }
        }
        TaskOp::SessionOn { id, s } => {
            let serial = find(id)?;
            target = Some(serial);
            let node = change.node(serial)?;
            if node.done {
                return Err(DhError::msg("That line is done; reopen it first"));
            }
            let on_it = node.meta.s.as_deref() == Some(s.as_str());
            if !on_it && meta_linked(&node.meta) {
                return Err(DhError::msg("Another session is on that line"));
            }
            change.unseat(s, Some(serial));
            if !on_it {
                let node = change.node_mut(serial)?;
                clear_link(&mut node.meta);
                node.meta.s = Some(s.clone());
                change.changed += 1;
            }
        }
        TaskOp::SessionOff { s } => {
            change.unseat(s, None);
        }
        TaskOp::TickLinked { link, auto, lift, from } => {
            let ticked = change.serials(|node| !node.done && linked_to(node, link));
            for serial in ticked {
                // Ticked meanwhile by a roll-up from a line ticked before it.
                if change.node(serial)?.done {
                    continue;
                }
                change.tick(serial, Some(*auto))?;
                if *lift == Some(true) {
                    let open: Vec<usize> = change
                        .node(serial)?
                        .lines()
                        .filter(|line| !line.done)
                        .map(|line| line.serial)
                        .collect();
                    let mut beside = serial;
                    for line in open {
                        let (mut node, _) = change.take(line)?;
                        if let Some(from) = from.as_deref().filter(|from| !from.is_empty()) {
                            node.meta.from = Some(from.to_owned());
                        }
                        change.place(
                            node,
                            Spot::Beside {
                                target: beside,
                                after: true,
                            },
                        )?;
                        change.changed += 1;
                        beside = line;
                    }
                }
                change.settle(change.parent_of(serial))?;
            }
        }
        TaskOp::ReopenLinked { link, auto } => {
            let reopened = change.serials(|node| node.done && node.meta.auto == Some(*auto) && linked_to(node, link));
            for serial in reopened {
                change.reopen(serial)?;
                change.settle(change.parent_of(serial))?;
            }
        }
        TaskOp::AbandonLinked { link, tried } => {
            let dropped = change.serials(|node| !node.done && linked_to(node, link));
            for serial in dropped {
                let node = change.node_mut(serial)?;
                clear_link(&mut node.meta);
                if let Some(tried) = tried.as_deref().filter(|tried| !tried.is_empty()) {
                    node.meta.tried = Some(tried.to_owned());
                }
                change.changed += 1;
            }
        }
    }
    let task = match (target, op) {
        (_, TaskOp::Remove { .. }) | (None, _) => None,
        (Some(serial), _) => change.node(serial).ok().map(Node::task),
    };
    Ok(TaskOpResult {
        text: serialize(&change.parts, Render::All),
        task,
        changed: change.changed,
    })
}

/// The top-level lines, as tasks (`tasks::parse_tasks`).
pub(crate) fn top_level_tasks(text: &str) -> Vec<Task> {
    parse_parts(text)
        .iter()
        .filter_map(|part| match part {
            Part::Line(node) => Some(node.task()),
            Part::Text(_) => None,
        })
        .collect()
}

// ── Reading the list ─────────────────────────────────────────────────────────────────────────────────────────────

/// One line of the list, as read.
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    /// `t=`, or (never written yet) a provisional `~<position>:<hash>` that ops accept until the next write.
    pub id: String,
    pub title: String,
    pub done: bool,
    /// Its details (indented text under it that isn't a line), common indent removed.
    pub body: String,
    pub meta: TaskMeta,
    /// The ID of the line it is under (`None`: top-level).
    pub parent: Option<String>,
    /// 0 for a top-level line.
    pub depth: usize,
}

impl Line {
    /// As a task (what the board and the protocol carry).
    pub fn task(&self) -> Task {
        Task {
            id: self.id.clone(),
            title: self.title.clone(),
            done: self.done,
            body: self.body.clone(),
            meta: self.meta.clone(),
        }
    }

    /// Someone is on it: a session (`s=`), or a worktree's sessions (legacy `wt=`).
    pub fn is_taken(&self) -> bool {
        meta_linked(&self.meta)
    }
}

/// Ticked and total lines directly under a line.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LineCount {
    pub ticked: usize,
    pub total: usize,
}

/// The whole list, in file order (a line's lines follow it).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TaskList {
    lines: Vec<Line>,
    /// Each line's parent, as an index into `lines` (always smaller than the line's own).
    parents: Vec<Option<usize>>,
}

impl TaskList {
    pub fn parse(text: &str) -> Self {
        let parts = parse_parts(text);
        let flat = flatten(&parts);
        let mut lines: Vec<Line> = Vec::with_capacity(flat.len());
        for item in &flat {
            let task = item.node.task();
            let parent = item.parent.and_then(|at| lines.get(at)).map(|line| line.id.clone());
            lines.push(Line {
                id: task.id,
                title: task.title,
                done: task.done,
                body: task.body,
                meta: task.meta,
                parent,
                depth: item.depth,
            });
        }
        let parents = flat.iter().map(|item| item.parent).collect();
        TaskList { lines, parents }
    }

    pub fn lines(&self) -> &[Line] {
        &self.lines
    }

    fn index(&self, id: &str) -> Option<usize> {
        self.lines.iter().position(|line| line.id == id)
    }

    pub fn get(&self, id: &str) -> Option<&Line> {
        self.index(id).and_then(|at| self.lines.get(at))
    }

    pub fn parent(&self, id: &str) -> Option<&Line> {
        let at = self.index(id)?;
        self.parents
            .get(at)
            .copied()
            .flatten()
            .and_then(|at| self.lines.get(at))
    }

    /// Its parent, that line's parent, and so on to the top level.
    pub fn ancestors(&self, id: &str) -> Vec<&Line> {
        let mut out = Vec::new();
        let mut current = self.index(id).and_then(|at| self.parents.get(at).copied().flatten());
        while let Some(at) = current {
            let Some(line) = self.lines.get(at) else { break };
            out.push(line);
            // Parents come before their lines, so this always moves up the file and ends.
            current = self.parents.get(at).copied().flatten().filter(|parent| *parent < at);
        }
        out
    }

    fn children_of(&self, parent: Option<usize>) -> Vec<&Line> {
        self.lines
            .iter()
            .zip(&self.parents)
            .filter(|(_, of)| **of == parent)
            .map(|(line, _)| line)
            .collect()
    }

    /// The lines directly under it, in order.
    pub fn children(&self, id: &str) -> Vec<&Line> {
        match self.index(id) {
            Some(at) => self.children_of(Some(at)),
            None => Vec::new(),
        }
    }

    pub fn top_level(&self) -> Vec<&Line> {
        self.children_of(None)
    }

    /// The line and every line under it, at any depth, in file order.
    pub fn subtree(&self, id: &str) -> Vec<&Line> {
        let Some(root) = self.index(id) else {
            return Vec::new();
        };
        let lines = self.lines.get(root..).unwrap_or_default();
        let depth = lines.first().map_or(0, |line| line.depth);
        // File order puts a line's lines right after it: the subtree ends at the first line no deeper than it.
        let end = lines
            .iter()
            .skip(1)
            .position(|line| line.depth <= depth)
            .map_or(lines.len(), |at| at + 1);
        lines.iter().take(end).collect()
    }

    /// `ticked/total` of the lines directly under it.
    pub fn count(&self, id: &str) -> LineCount {
        let children = self.children(id);
        LineCount {
            ticked: children.iter().filter(|line| line.done).count(),
            total: children.len(),
        }
    }

    /// The line session `s` is on (the first, should a hand edit have put it on more).
    pub fn line_of_session(&self, s: &str) -> Option<&Line> {
        self.lines.iter().find(|line| line.meta.s.as_deref() == Some(s))
    }

    /// The backlog: open top-level lines nobody is on.
    pub fn backlog(&self) -> Vec<&Line> {
        self.top_level()
            .into_iter()
            .filter(|line| !line.done && !line.is_taken())
            .collect()
    }

    /// Open lines, at any depth.
    pub fn open_count(&self) -> usize {
        self.lines.iter().filter(|line| !line.done).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tasks::{apply_task_op_with, parse_tasks};
    use serde_json::{Value, json};

    fn now() -> i64 {
        jsstr::date::parse("2026-11-14T12:00:00Z").unwrap_or(0)
    }

    fn op(value: Value) -> TaskOp {
        serde_json::from_value(value).unwrap_or_else(|error| panic!("bad op: {error}"))
    }

    /// The op's result, new IDs taken from `n1`, `n2`, ...
    fn run(text: &str, value: Value) -> TaskOpResult {
        let mut n = 0;
        let mut next = || {
            n += 1;
            format!("n{n}")
        };
        apply_task_op_with(text, &op(value), now(), &mut next).unwrap_or_else(|error| panic!("op failed: {error}"))
    }

    fn apply(text: &str, value: Value) -> String {
        run(text, value).text
    }

    fn fails(text: &str, value: Value) -> String {
        let mut next = || "x".to_owned();
        match apply_task_op_with(text, &op(value), now(), &mut next) {
            Ok(result) => panic!("expected an error, got {:?}", result.text),
            Err(error) => error.to_string(),
        }
    }

    /// Each line as `<depth spaces><title>` plus `[x]` when ticked, `^<parent title>` checked against the parents.
    fn tree(text: &str) -> Vec<String> {
        let list = TaskList::parse(text);
        list.lines()
            .iter()
            .map(|line| {
                let parent = list.parent(&line.id).map(|parent| parent.title.clone());
                let expected = list.ancestors(&line.id).first().map(|parent| parent.title.clone());
                assert_eq!(parent, expected, "parent of {}", line.title);
                format!(
                    "{}{}{}",
                    "  ".repeat(line.depth),
                    line.title,
                    if line.done { " [x]" } else { "" }
                )
            })
            .collect()
    }

    fn line<'a>(list: &'a TaskList, id: &str) -> &'a Line {
        list.get(id).unwrap_or_else(|| panic!("no line {id}"))
    }

    const IDEA: &str = "\
# Ideas

- [ ] Rework auth <!-- dh:t=idea s=orch -->
  why: tokens leak
  - [ ] Token storage <!-- dh:t=tok s=s-tok -->
    - [x] schema <!-- dh:t=sch done=2026-11-01 -->
    - [ ] encryption <!-- dh:t=enc -->
    - [ ] refresh tokens <!-- dh:t=ref s=s-ref -->
  - [ ] Login UI <!-- dh:t=ui s=s-ui -->
  - [ ] Remove legacy endpoint <!-- dh:t=leg -->
- [ ] Backlog item <!-- dh:t=back -->

Notes at the end.
";

    #[test]
    fn nested_checklist_items_are_lines_under_lines() {
        assert_eq!(
            tree(IDEA),
            [
                "Rework auth",
                "  Token storage",
                "    schema [x]",
                "    encryption",
                "    refresh tokens",
                "  Login UI",
                "  Remove legacy endpoint",
                "Backlog item",
            ]
        );
        let list = TaskList::parse(IDEA);
        // Details are the line's own text, not the lines under it.
        assert_eq!(line(&list, "idea").body, "why: tokens leak");
        assert_eq!(line(&list, "tok").parent.as_deref(), Some("idea"));
        assert_eq!(line(&list, "sch").depth, 2);
        // The top-level view (the board, until it shows the tree) has the top-level lines only.
        let titles: Vec<String> = parse_tasks(IDEA).into_iter().map(|task| task.title).collect();
        assert_eq!(titles, ["Rework auth", "Backlog item"]);
    }

    #[test]
    fn a_nested_file_deckhand_wrote_round_trips_byte_identical() {
        assert_eq!(serialize(&parse_parts(IDEA), Render::All), IDEA);
        // An op that changes nothing writes the file back exactly.
        let result = run(IDEA, json!({"type": "session-off", "s": "nobody"}));
        assert_eq!(result.changed, 0);
        assert_eq!(result.text, IDEA);
        // And an op on one line leaves every other byte alone.
        let ticked = apply(IDEA, json!({"type": "tick", "id": "leg"}));
        let changed: Vec<(&str, &str)> = IDEA
            .split('\n')
            .zip(ticked.split('\n'))
            .filter(|(before, after)| before != after)
            .collect();
        assert_eq!(
            changed,
            [(
                "  - [ ] Remove legacy endpoint <!-- dh:t=leg -->",
                "  - [x] Remove legacy endpoint <!-- dh:t=leg done=2026-11-14 -->"
            )]
        );
    }

    #[test]
    fn a_legacy_file_reads_as_it_did() {
        let legacy = "# Tasks\n- [ ] Ship it <!-- dh:t=a wt=w1 added=2026-10-01 -->\n  why:\n\n- [x] Old <!-- dh:t=b s=s1 done=2026-10-02 auto=merge -->\n  detail\n    deeper\nprose\n";
        let tasks = parse_tasks(legacy);
        let shape: Vec<(&str, &str, Option<&str>)> = tasks
            .iter()
            .map(|task| (task.title.as_str(), task.body.as_str(), task.meta.wt.as_deref()))
            .collect();
        assert_eq!(
            shape,
            [("Ship it", "why:", Some("w1")), ("Old", "detail\n  deeper", None)]
        );
        assert_eq!(tree(legacy), ["Ship it", "Old [x]"]);
        // A change elsewhere keeps it as written, worktree link included.
        let after = apply(legacy, json!({"type": "add", "title": "New", "id": "c"}));
        assert_eq!(
            after,
            "# Tasks\n- [ ] Ship it <!-- dh:t=a wt=w1 added=2026-10-01 -->\n  why:\n- [ ] New <!-- dh:t=c added=2026-11-14 -->\n\n- [x] Old <!-- dh:t=b s=s1 done=2026-10-02 auto=merge -->\n  detail\n    deeper\nprose\n"
        );
    }

    #[test]
    fn worktree_links_are_read_and_kept_but_list_ops_never_write_one() {
        let text = "- [ ] Ship it <!-- dh:t=a wt=w1 -->\n  - [ ] step <!-- dh:t=b -->\n- [ ] Other <!-- dh:t=c -->\n";
        let list = TaskList::parse(text);
        assert_eq!(line(&list, "a").meta.wt.as_deref(), Some("w1"));
        assert!(line(&list, "a").is_taken());
        // Matched by the linked-work ops, as before.
        let merged = run(
            text,
            json!({"type": "tick-linked", "link": {"wt": "w1"}, "auto": "merge"}),
        );
        assert_eq!(merged.changed, 1);
        assert!(
            merged
                .text
                .starts_with("- [x] Ship it <!-- dh:t=a wt=w1 done=2026-11-14 auto=merge -->")
        );
        // The list's own ops add none: a new line, a move, a lift, a session.
        let mut after = apply(text, json!({"type": "add", "title": "Under", "parent": "a"}));
        after = apply(&after, json!({"type": "move-to", "id": "c", "parent": "a"}));
        after = apply(
            &after,
            json!({"type": "tick-linked", "link": {"wt": "w1"}, "auto": "merge", "lift": true, "from": "feat"}),
        );
        assert_eq!(after.matches("wt=").count(), 1, "{after}");
        // A session taking the line on replaces the worktree link.
        let on = apply(text, json!({"type": "session-on", "id": "b", "s": "s1"}));
        let taken = apply(&on, json!({"type": "session-off", "s": "s1"}));
        assert!(!apply(&taken, json!({"type": "link", "id": "a", "link": {"s": "s2"}})).contains("wt="));
    }

    #[test]
    fn every_line_gets_an_id_on_the_first_write() {
        let text = "- [ ] Top\n  - [ ] under\n    - [x] deeper\n";
        let ids: Vec<String> = TaskList::parse(text)
            .lines()
            .iter()
            .map(|line| line.id.clone())
            .collect();
        // Before: provisional IDs, by position among all lines; ops accept them.
        assert!(
            ids.iter()
                .enumerate()
                .all(|(at, id)| id.starts_with(&format!("~{at}:"))),
            "{ids:?}"
        );
        let after = apply(text, json!({"type": "tick", "id": ids[1]}));
        assert_eq!(
            after,
            "- [x] Top <!-- dh:t=n1 done=2026-11-14 auto=lines -->\n  - [x] under <!-- dh:t=n2 done=2026-11-14 -->\n    - [x] deeper <!-- dh:t=n3 -->\n"
        );
    }

    #[test]
    fn add_puts_a_line_under_its_parent_at_the_end_or_a_position() {
        let text = "- [ ] A <!-- dh:t=a -->\n  - [ ] one <!-- dh:t=1 -->\n  - [ ] two <!-- dh:t=2 -->\n- [x] Done <!-- dh:t=d -->\n";
        let last = apply(
            text,
            json!({"type": "add", "title": "three", "parent": "a", "body": "why\n\nbecause"}),
        );
        assert_eq!(
            last,
            "- [ ] A <!-- dh:t=a -->\n  - [ ] one <!-- dh:t=1 -->\n  - [ ] two <!-- dh:t=2 -->\n  - [ ] three <!-- dh:t=n1 added=2026-11-14 -->\n    why\n    \n    because\n- [x] Done <!-- dh:t=d -->\n"
        );
        assert_eq!(
            TaskList::parse(&last).get("n1").map(|line| line.body.as_str()),
            Some("why\n\nbecause")
        );
        let first = apply(text, json!({"type": "add", "title": "zero", "parent": "a", "at": 0}));
        assert_eq!(tree(&first), ["A", "  zero", "  one", "  two", "Done [x]"]);
        // Under a line with nothing under it yet: two spaces in.
        let deeper = apply(text, json!({"type": "add", "title": "half", "parent": "1"}));
        assert!(deeper.contains("\n    - [ ] half <!--"), "{deeper}");
        // Top-level: the backlog's end (after the last open top-level line), or a position.
        assert_eq!(
            tree(&apply(text, json!({"type": "add", "title": "B"}))),
            ["A", "  one", "  two", "B", "Done [x]"]
        );
        assert_eq!(
            tree(&apply(text, json!({"type": "add", "title": "B", "at": 0}))),
            ["B", "A", "  one", "  two", "Done [x]"]
        );
        assert_eq!(
            tree(&apply(text, json!({"type": "add", "title": "B", "at": 9}))),
            ["A", "  one", "  two", "Done [x]", "B"]
        );
        // A file indented four spaces a level keeps its indent.
        let wide = "- [ ] A <!-- dh:t=a -->\n    - [ ] one <!-- dh:t=1 -->\n";
        assert!(apply(wide, json!({"type": "add", "title": "two", "parent": "a"})).contains("\n    - [ ] two <!--"));
        assert!(fails(text, json!({"type": "add", "title": "x", "parent": "gone"})).contains("no longer in the list"));
    }

    #[test]
    fn edit_and_retitle_leave_the_lines_under_it() {
        let text = "- [ ] A <!-- dh:t=a -->\n  old detail\n  - [ ] one <!-- dh:t=1 -->\n  more old\n  - [ ] two <!-- dh:t=2 -->\n";
        let edited = apply(
            text,
            json!({"type": "edit", "id": "a", "title": "A2", "body": "new detail"}),
        );
        assert_eq!(
            edited,
            "- [ ] A2 <!-- dh:t=a -->\n  new detail\n  - [ ] one <!-- dh:t=1 -->\n  - [ ] two <!-- dh:t=2 -->\n"
        );
        let nested = apply(text, json!({"type": "edit", "id": "1", "title": "one", "body": "a\nb"}));
        assert!(
            nested.contains("  - [ ] one <!-- dh:t=1 -->\n    a\n    b\n  more old\n"),
            "{nested}"
        );
        let retitled = apply(text, json!({"type": "retitle", "id": "2", "title": "  two  again "}));
        assert!(retitled.contains("  - [ ] two again <!-- dh:t=2 -->"), "{retitled}");
        assert!(retitled.contains("old detail"));
        assert!(fails(text, json!({"type": "retitle", "id": "2", "title": " "})).contains("needs a title"));
    }

    #[test]
    fn tick_reopen_and_toggle() {
        let text = "- [ ] A <!-- dh:t=a -->\n- [x] B <!-- dh:t=b done=2026-11-01 auto=merge -->\n";
        let ticked = run(text, json!({"type": "tick", "id": "a"}));
        assert_eq!(ticked.changed, 1);
        assert!(ticked.text.starts_with("- [x] A <!-- dh:t=a done=2026-11-14 -->"));
        assert_eq!(run(&ticked.text, json!({"type": "tick", "id": "a"})).changed, 0);
        let reopened = run(text, json!({"type": "reopen", "id": "b"}));
        assert!(reopened.text.ends_with("- [ ] B <!-- dh:t=b -->\n"));
        assert_eq!(run(text, json!({"type": "reopen", "id": "a"})).changed, 0);
        assert_eq!(
            apply(text, json!({"type": "toggle", "id": "b"})),
            "- [ ] A <!-- dh:t=a -->\n- [ ] B <!-- dh:t=b -->\n"
        );
    }

    #[test]
    fn a_line_ticks_itself_when_its_last_line_ticks_and_reopens_with_one() {
        let text = "- [ ] Idea <!-- dh:t=i -->\n  - [ ] Task <!-- dh:t=t -->\n    - [x] a <!-- dh:t=a -->\n    - [ ] b <!-- dh:t=b -->\n  - [x] Other <!-- dh:t=o -->\n";
        let result = run(text, json!({"type": "tick", "id": "b"}));
        // b, then Task (its lines all ticked), then Idea (likewise).
        assert_eq!(result.changed, 3);
        assert_eq!(
            result.text,
            "- [x] Idea <!-- dh:t=i done=2026-11-14 auto=lines -->\n  - [x] Task <!-- dh:t=t done=2026-11-14 auto=lines -->\n    - [x] a <!-- dh:t=a -->\n    - [x] b <!-- dh:t=b done=2026-11-14 -->\n  - [x] Other <!-- dh:t=o -->\n"
        );
        // Reopening one of its lines reopens what the roll-up ticked, all the way up.
        let reopened = apply(&result.text, json!({"type": "reopen", "id": "a"}));
        assert_eq!(tree(&reopened), ["Idea", "  Task", "    a", "    b [x]", "  Other [x]"]);
        // A line ticked by hand (or by a merge) stays ticked.
        let by_hand = "- [x] Task <!-- dh:t=t done=2026-11-01 -->\n  - [x] a <!-- dh:t=a -->\n";
        assert_eq!(
            tree(&apply(by_hand, json!({"type": "reopen", "id": "a"}))),
            ["Task [x]", "  a"]
        );
        let merged = "- [x] Task <!-- dh:t=t done=2026-11-01 auto=merge -->\n  - [x] a <!-- dh:t=a -->\n";
        assert_eq!(
            tree(&apply(merged, json!({"type": "reopen", "id": "a"}))),
            ["Task [x]", "  a"]
        );
        // Ticking a line by hand leaves the lines under it as they are (and still counts for the line above).
        assert_eq!(
            tree(&apply(text, json!({"type": "tick", "id": "t"}))),
            ["Idea [x]", "  Task [x]", "    a [x]", "    b", "  Other [x]"]
        );
    }

    #[test]
    fn new_or_moved_in_open_lines_reopen_a_rolled_up_line() {
        let text = "- [x] Task <!-- dh:t=t done=2026-11-01 auto=lines -->\n  - [x] a <!-- dh:t=a -->\n- [ ] Loose <!-- dh:t=l -->\n";
        assert_eq!(
            tree(&apply(text, json!({"type": "add", "title": "b", "parent": "t"}))),
            ["Task", "  a [x]", "  b", "Loose"]
        );
        assert_eq!(
            tree(&apply(text, json!({"type": "move-to", "id": "l", "parent": "t"}))),
            ["Task", "  a [x]", "  Loose"]
        );
        // Removing or moving out the last open line ticks the line it was under.
        let open = "- [ ] Task <!-- dh:t=t -->\n  - [x] a <!-- dh:t=a -->\n  - [ ] b <!-- dh:t=b -->\n";
        assert_eq!(
            tree(&apply(open, json!({"type": "remove", "id": "b"}))),
            ["Task [x]", "  a [x]"]
        );
        assert_eq!(
            tree(&apply(open, json!({"type": "move-to", "id": "b"}))),
            ["Task [x]", "  a [x]", "b"]
        );
    }

    #[test]
    fn removing_a_line_removes_everything_under_it() {
        let removed = run(IDEA, json!({"type": "remove", "id": "tok"}));
        assert!(removed.task.is_none());
        assert_eq!(
            tree(&removed.text),
            ["Rework auth", "  Login UI", "  Remove legacy endpoint", "Backlog item"]
        );
        assert!(!removed.text.contains("schema"));
        assert!(removed.text.contains("  why: tokens leak\n"));
        assert!(removed.text.ends_with("\nNotes at the end.\n"));
    }

    #[test]
    fn a_line_moves_with_its_lines_and_details() {
        let text = "- [ ] A <!-- dh:t=a -->\n  - [ ] Sub <!-- dh:t=s -->\n    detail\n    - [ ] leaf <!-- dh:t=l -->\n- [ ] B <!-- dh:t=b -->\n";
        let to_b = apply(text, json!({"type": "move-to", "id": "s", "parent": "b"}));
        assert_eq!(
            to_b,
            "- [ ] A <!-- dh:t=a -->\n- [ ] B <!-- dh:t=b -->\n  - [ ] Sub <!-- dh:t=s -->\n    detail\n    - [ ] leaf <!-- dh:t=l -->\n"
        );
        let to_top = apply(text, json!({"type": "move-to", "id": "s", "at": 0}));
        assert_eq!(
            to_top,
            "- [ ] Sub <!-- dh:t=s -->\n  detail\n  - [ ] leaf <!-- dh:t=l -->\n- [ ] A <!-- dh:t=a -->\n- [ ] B <!-- dh:t=b -->\n"
        );
        let under_leaf_of_other = apply(text, json!({"type": "move-to", "id": "b", "parent": "l"}));
        assert_eq!(tree(&under_leaf_of_other), ["A", "  Sub", "    leaf", "      B"]);
        // Beside a line in another parent: into that parent.
        let beside = apply(text, json!({"type": "move", "id": "b", "target": "l"}));
        assert_eq!(tree(&beside), ["A", "  Sub", "    B", "    leaf"]);
        let after = apply(text, json!({"type": "move", "id": "b", "target": "s", "after": true}));
        assert_eq!(tree(&after), ["A", "  Sub", "    leaf", "  B"]);
    }

    #[test]
    fn a_line_cannot_move_under_itself() {
        let text = "- [ ] A <!-- dh:t=a -->\n  - [ ] Sub <!-- dh:t=s -->\n    - [ ] leaf <!-- dh:t=l -->\n";
        for value in [
            json!({"type": "move-to", "id": "a", "parent": "l"}),
            json!({"type": "move-to", "id": "s", "parent": "s"}),
            json!({"type": "move", "id": "a", "target": "s"}),
        ] {
            assert!(fails(text, value).contains("can't move under itself"));
        }
        // Beside itself: nothing to do.
        assert_eq!(run(text, json!({"type": "move", "id": "s", "target": "s"})).changed, 0);
    }

    #[test]
    fn reorder_moves_a_line_among_its_siblings() {
        let text = "- [ ] A <!-- dh:t=a -->\n  - [ ] one <!-- dh:t=1 -->\n    - [ ] deep <!-- dh:t=d -->\n  note\n  - [ ] two <!-- dh:t=2 -->\n- [ ] B <!-- dh:t=b -->\n";
        let down = apply(text, json!({"type": "reorder", "id": "1", "down": true}));
        assert_eq!(tree(&down), ["A", "  two", "  one", "    deep", "B"]);
        assert_eq!(
            TaskList::parse(&down).get("a").map(|line| line.body.as_str()),
            Some("note")
        );
        assert_eq!(
            tree(&apply(text, json!({"type": "reorder", "id": "2", "down": false}))),
            tree(&down)
        );
        assert_eq!(
            tree(&apply(text, json!({"type": "reorder", "id": "b", "down": false}))),
            ["B", "A", "  one", "    deep", "  two"]
        );
        // At either end: nothing changes.
        assert_eq!(
            run(text, json!({"type": "reorder", "id": "2", "down": true})).changed,
            0
        );
        assert_eq!(
            run(text, json!({"type": "reorder", "id": "a", "down": false})).changed,
            0
        );
    }

    #[test]
    fn a_session_is_on_one_line_at_most() {
        let text = "- [ ] A <!-- dh:t=a s=s1 -->\n- [x] B <!-- dh:t=b s=s1 done=2026-11-01 -->\n- [ ] C <!-- dh:t=c tried=old from=x -->\n- [ ] D <!-- dh:t=d s=s2 -->\n";
        // Putting it on a line takes it off any other, ticked or not; the new line's old attempt goes.
        let on = run(text, json!({"type": "session-on", "id": "c", "s": "s1"}));
        assert_eq!(on.changed, 3);
        assert_eq!(
            on.text,
            "- [ ] A <!-- dh:t=a -->\n- [x] B <!-- dh:t=b done=2026-11-01 -->\n- [ ] C <!-- dh:t=c s=s1 -->\n- [ ] D <!-- dh:t=d s=s2 -->\n"
        );
        assert_eq!(
            TaskList::parse(&on.text)
                .line_of_session("s1")
                .map(|line| line.id.as_str()),
            Some("c")
        );
        // Already on it: nothing changes.
        assert_eq!(
            run(&on.text, json!({"type": "session-on", "id": "c", "s": "s1"})).changed,
            0
        );
        // Someone else's line, or a done one: refused.
        assert!(fails(text, json!({"type": "session-on", "id": "d", "s": "s1"})).contains("Another session"));
        assert!(fails(text, json!({"type": "session-on", "id": "b", "s": "s3"})).contains("done"));
        // The other ways of putting a session on a line follow the same rule.
        for value in [
            json!({"type": "link", "id": "c", "link": {"s": "s1"}}),
            json!({"type": "assign", "id": "c", "link": {"s": "s1"}}),
        ] {
            assert_eq!(apply(text, value).matches("s=s1").count(), 1);
        }
        let added = apply(text, json!({"type": "add", "title": "E", "link": {"s": "s1"}}));
        assert_eq!(
            TaskList::parse(&added)
                .line_of_session("s1")
                .map(|line| line.title.as_str()),
            Some("E")
        );
        assert_eq!(added.matches("s=s1").count(), 1);
        // Off: nobody is on it, nothing else changes.
        let off = run(&on.text, json!({"type": "session-off", "s": "s1"}));
        assert_eq!(off.changed, 1);
        assert!(off.text.contains("- [ ] C <!-- dh:t=c -->\n"));
    }

    #[test]
    fn a_merge_lifts_its_open_lines_to_the_line_above() {
        let tick = |text: &str, lift: bool| {
            apply(
                text,
                json!({"type": "tick-linked", "link": {"s": "s-tok"}, "auto": "merge", "lift": lift, "from": "feat/tokens"}),
            )
        };
        let lifted = tick(IDEA, true);
        assert_eq!(
            tree(&lifted),
            [
                "Rework auth",
                "  Token storage [x]",
                "    schema [x]",
                "  encryption",
                "  refresh tokens",
                "  Login UI",
                "  Remove legacy endpoint",
                "Backlog item",
            ]
        );
        let list = TaskList::parse(&lifted);
        assert_eq!(line(&list, "tok").meta.auto, Some(TaskAuto::Merge));
        assert_eq!(line(&list, "enc").meta.from.as_deref(), Some("feat/tokens"));
        // A lifted line keeps who is on it.
        assert_eq!(line(&list, "ref").meta.s.as_deref(), Some("s-ref"));
        assert_eq!(line(&list, "sch").meta.from, None);
        assert!(
            lifted.contains("\n  - [ ] encryption <!-- dh:t=enc from=feat/tokens -->\n"),
            "{lifted}"
        );
        // Without `lift` they stay where they are.
        assert_eq!(
            tree(&tick(IDEA, false))[2..5],
            ["    schema [x]", "    encryption", "    refresh tokens"]
        );
        // A top-level line's open lines go to the top level, after it, with what is under them.
        let top = "- [ ] Ship <!-- dh:t=s s=s1 -->\n  - [ ] step <!-- dh:t=a -->\n    detail\n    - [x] sub <!-- dh:t=b -->\n- [ ] Next <!-- dh:t=n -->\n";
        let lifted = apply(
            top,
            json!({"type": "tick-linked", "link": {"s": "s1"}, "auto": "done", "lift": true, "from": "main"}),
        );
        assert_eq!(
            lifted,
            "- [x] Ship <!-- dh:t=s s=s1 done=2026-11-14 auto=done -->\n- [ ] step <!-- dh:t=a from=main -->\n  detail\n  - [x] sub <!-- dh:t=b -->\n- [ ] Next <!-- dh:t=n -->\n"
        );
    }

    #[test]
    fn a_merge_rolls_up_and_undoing_it_rolls_back() {
        let text = "- [ ] Idea <!-- dh:t=i -->\n  - [x] One <!-- dh:t=1 -->\n  - [ ] Two <!-- dh:t=2 s=s2 -->\n";
        let merged = apply(
            text,
            json!({"type": "tick-linked", "link": {"s": "s2"}, "auto": "merge"}),
        );
        assert_eq!(tree(&merged), ["Idea [x]", "  One [x]", "  Two [x]"]);
        assert_eq!(
            TaskList::parse(&merged).get("i").and_then(|line| line.meta.auto),
            Some(TaskAuto::Lines)
        );
        let undone = run(
            &merged,
            json!({"type": "reopen-linked", "link": {"s": "s2"}, "auto": "merge"}),
        );
        assert_eq!(undone.changed, 2);
        assert_eq!(tree(&undone.text), ["Idea", "  One [x]", "  Two"]);
    }

    #[test]
    fn abandoning_unhooks_a_line_where_it_is() {
        let result = run(
            IDEA,
            json!({"type": "abandon-linked", "link": {"s": "s-ref"}, "tried": "feat/refresh"}),
        );
        assert_eq!(result.changed, 1);
        assert_eq!(tree(&result.text), tree(IDEA));
        let list = TaskList::parse(&result.text);
        let refresh = line(&list, "ref");
        assert_eq!(
            (refresh.meta.s.as_deref(), refresh.meta.tried.as_deref()),
            (None, Some("feat/refresh"))
        );
        assert_eq!(refresh.parent.as_deref(), Some("tok"));
    }

    #[test]
    fn queries_answer_from_the_tree() {
        let list = TaskList::parse(IDEA);
        let ids = |lines: Vec<&Line>| lines.into_iter().map(|line| line.id.clone()).collect::<Vec<_>>();
        assert_eq!(list.parent("enc").map(|line| line.id.as_str()), Some("tok"));
        assert_eq!(list.parent("idea"), None);
        assert_eq!(ids(list.ancestors("ref")), ["tok", "idea"]);
        assert_eq!(ids(list.children("idea")), ["tok", "ui", "leg"]);
        assert_eq!(ids(list.children("sch")), Vec::<String>::new());
        assert_eq!(ids(list.subtree("tok")), ["tok", "sch", "enc", "ref"]);
        assert_eq!(ids(list.subtree("gone")), Vec::<String>::new());
        assert_eq!(ids(list.top_level()), ["idea", "back"]);
        assert_eq!(list.count("tok"), LineCount { ticked: 1, total: 3 });
        assert_eq!(list.count("idea"), LineCount { ticked: 0, total: 3 });
        assert_eq!(list.line_of_session("s-ui").map(|line| line.id.as_str()), Some("ui"));
        assert_eq!(list.line_of_session("nobody"), None);
        assert_eq!(ids(list.backlog()), ["back"]);
        assert_eq!(list.open_count(), 7);
    }

    #[test]
    fn odd_indentation_parses_by_columns() {
        let text = [
            "- [ ] Four spaces <!-- dh:t=a -->",
            "    - [ ] one",
            "        - [ ] under one",
            "    - [ ] two",
            "\t- [ ] tab is four columns: a sibling",
            "  - [ ] shallower than its siblings: still under the top line",
            "     - [ ] deeper than that one: under it",
            "    detail under the shallower one",
            "  detail of the top line",
            "      - [ ] after the top line's detail: under the top line",
            "",
            "  - [ ] after a blank line: prose",
            "- [ ] Next <!-- dh:t=b -->",
            "   ",
            "   - [ ] after a line of spaces: still in the block",
            "Unindented prose",
            "  - [ ] indented under prose: prose",
        ]
        .join("\n");
        assert_eq!(
            tree(&text),
            [
                "Four spaces",
                "  one",
                "    under one",
                "  two",
                "  tab is four columns: a sibling",
                "  shallower than its siblings: still under the top line",
                "    deeper than that one: under it",
                "  after the top line's detail: under the top line",
                "Next",
                "  after a line of spaces: still in the block",
            ]
        );
        let list = TaskList::parse(&text);
        let shallower = &list.lines()[5];
        assert_eq!(shallower.body, "detail under the shallower one");
        assert_eq!(line(&list, "a").body, "detail of the top line");
        // Everything reads back as written.
        assert_eq!(serialize(&parse_parts(&text), Render::TopLevel), format!("{text}\n"));
    }

    #[test]
    fn changes_to_oddly_indented_files_keep_the_tree() {
        // Siblings at different indents: taking the detail between them out must not put `deep` under `one`.
        let text =
            "- [ ] A <!-- dh:t=a -->\n  - [ ] one <!-- dh:t=1 -->\n  between\n      - [ ] deep <!-- dh:t=d -->\n";
        assert_eq!(tree(text), ["A", "  one", "  deep"]);
        let edited = apply(text, json!({"type": "edit", "id": "a", "title": "A", "body": ""}));
        assert_eq!(tree(&edited), ["A", "  one", "  deep"]);
        // Removing the line before a more indented sibling.
        let shallow = "- [ ] A <!-- dh:t=a -->\n    - [ ] one <!-- dh:t=1 -->\n  - [ ] two <!-- dh:t=2 -->\n  detail\n      - [ ] three <!-- dh:t=3 -->\n";
        assert_eq!(tree(shallow), ["A", "  one", "  two", "  three"]);
        let removed = apply(shallow, json!({"type": "remove", "id": "2"}));
        assert_eq!(tree(&removed), ["A", "  one", "  three"]);
        assert_eq!(
            TaskList::parse(&removed).get("a").map(|line| line.body.as_str()),
            Some("detail")
        );
        // Moving a tab-indented line with its details to the top level and back under another line.
        let tabs = "- [ ] A <!-- dh:t=a -->\n\t- [ ] Sub <!-- dh:t=s -->\n\t\tdetail\n\t\t- [ ] leaf <!-- dh:t=l -->\n- [ ] B <!-- dh:t=b -->\n";
        let top = apply(tabs, json!({"type": "move-to", "id": "s", "at": 1}));
        assert_eq!(tree(&top), ["A", "Sub", "  leaf", "B"]);
        assert!(
            top.contains("- [ ] Sub <!-- dh:t=s -->\n    detail\n    - [ ] leaf"),
            "{top}"
        );
        let back = apply(&top, json!({"type": "move-to", "id": "s", "parent": "b"}));
        assert_eq!(tree(&back), ["A", "B", "  Sub", "    leaf"]);
        assert_eq!(
            TaskList::parse(&back).get("s").map(|line| line.body.as_str()),
            Some("detail")
        );
    }

    #[test]
    fn a_copied_id_addresses_the_first_line() {
        let text = "- [ ] First <!-- dh:t=same -->\n  - [ ] child <!-- dh:t=c -->\n- [ ] Copy <!-- dh:t=same -->\n";
        let list = TaskList::parse(text);
        assert_eq!(line(&list, "same").title, "First");
        assert_eq!(list.children("same").len(), 1);
        assert_eq!(
            tree(&apply(text, json!({"type": "tick", "id": "same"}))),
            ["First [x]", "  child", "Copy"]
        );
        assert!(fails(text, json!({"type": "add", "title": "x", "id": "same"})).contains("already exists"));
    }
}
