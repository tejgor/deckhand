//! The work-items migration (`docs/future/work-items.md`, *Migration*): one repository's legacy list and notes in, the
//! nested list and line notes out. Pure: texts and facts from the records in, texts out; the daemon reads the files,
//! backs them up, writes what this answers and retires the old notes.
//!
//! # What it does, in order
//!
//! 1. **Who is on each line.** In file order: a line with `s=` keeps its session; a line with `wt=` moves onto the
//!    worktree's owning session ([`owning_session`]); a worktree deleted without merging unhooks its line and marks it
//!    `tried=<branch>`; a worktree with no session left unhooks it. A session already on a line further up the file
//!    stays there, and this line is unhooked. `wt=` is gone from every line afterwards.
//! 2. **Details become line notes.** Every line's details (indented text under it that isn't a line) leave the list
//!    and start that line's note (`notes/lines/<line id>.md`).
//! 3. **Notes become lines and line notes.** A worktree's note goes to the line its owning session is on, else the
//!    first line that had `wt=` for it; a session's note (a main-checkout session's, or a worktree session's leftover
//!    own note) to the line that session is on. A note without a line gets one: for its session, titled after it,
//!    under its parent session's line when that has one (after that line's own steps), else at the top level (where
//!    `add` puts a line); for a worktree with no session left, titled after its branch, nobody on it. A session in a
//!    worktree deleted unmerged gets its line unhooked and marked `tried`. Checkboxes become lines under it (ticked
//!    state and nesting kept; text indented under a checkbox, up to a blank line, becomes that line's note), prose and
//!    headings are appended to its note after a blank line, `↗` lines are dropped. A note with nothing but blank lines
//!    (or `↗` lines) makes nothing; a note of a session the records don't know stays where it is.
//! 4. **Merged work.** The open lines directly under a line whose worktree was merged (with what is under them) move
//!    to the top level, right after the line's top-level line, marked `from=<branch>`; a line created here for merged
//!    work is ticked as merging ticks it (`auto=merge`, dated when it merged), or removed again when nothing is left
//!    under it.
//!
//! Nothing in the input is lost: every line of details and notes that isn't blank ends up in the list or a line note,
//! except `↗` lines, which the report lists (a checkbox's text is its line's title, and its note when the title can't
//! hold it as written). Bookkeeping keys the list no longer writes (`assigned=`, `was=`, unknown ones) go, as with any
//! write of the list. Nothing rolls up. Running it on its own output changes nothing. New IDs come from the generator
//! passed in (skipping any already in use), so a run is reproducible.

use std::collections::{HashMap, HashSet};
use std::fmt;

use deckhand_protocol::data::{SessionRecord, SessionWorktreeMode, TaskAuto, TaskMeta, WorktreeOrigin};
use deckhand_protocol::records::WorktreeRecord;

use crate::error::Result;
use crate::jsstr;
use crate::notes::parse_checklist_line;
use crate::task_list::{Editor, TaskList};
use crate::tasks::{clean_task_title, normalize_newlines, parse_note_task_link, without_task_meta};

/// What the migration needs to know about a session.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SessionFacts {
    pub id: String,
    pub title: String,
    /// The worktree record it works in (`worktree.id`).
    pub worktree: Option<String>,
    /// It made that worktree: Deckhand created it for this session (`worktree.origin` `created`, or a `managed`
    /// worktree recorded before origins were).
    pub created_worktree: bool,
    pub parent: Option<String>,
    /// ISO-8601, as recorded (orders sessions: the oldest first).
    pub created_at: String,
}

impl SessionFacts {
    pub fn from_record(session: &SessionRecord) -> Self {
        let worktree = session.worktree.as_ref();
        SessionFacts {
            id: session.id.clone(),
            title: session.title.clone(),
            worktree: worktree
                .and_then(|worktree| worktree.id.clone())
                .filter(|id| !id.is_empty()),
            created_worktree: worktree.is_some_and(|worktree| match worktree.origin {
                Some(origin) => origin == WorktreeOrigin::Created,
                None => worktree.mode == SessionWorktreeMode::Managed,
            }),
            parent: session.parent_session_id.clone().filter(|id| !id.is_empty()),
            created_at: session.created_at.clone(),
        }
    }
}

/// What the migration needs to know about a worktree record.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WorktreeFacts {
    pub id: String,
    pub branch: Option<String>,
    /// When it was merged (ISO-8601), if it was.
    pub merged_at: Option<String>,
    pub deleted: bool,
}

impl WorktreeFacts {
    /// Deleted without being merged: its work was dropped.
    fn dropped(&self) -> bool {
        self.deleted && self.merged_at.is_none()
    }
}

fn some_text(value: Option<&String>) -> Option<String> {
    value.filter(|text| !text.is_empty()).cloned()
}

/// The facts of every worktree the records know: each record, and each worktree ID only sessions still name (its
/// record gone), from what those sessions recorded. The branch is the one its sessions recorded (the owning session's
/// first), else the merge's source.
pub fn worktree_facts(records: &[WorktreeRecord], sessions: &[SessionRecord]) -> Vec<WorktreeFacts> {
    let facts: Vec<SessionFacts> = sessions.iter().map(SessionFacts::from_record).collect();
    let branch_of = |id: &str| -> Option<String> {
        let mut named: Vec<(&SessionFacts, &SessionRecord)> = facts
            .iter()
            .zip(sessions)
            .filter(|(fact, _)| fact.worktree.as_deref() == Some(id))
            .collect();
        named.sort_by(|(left, _), (right, _)| owner_order(left, right));
        named
            .iter()
            .find_map(|(_, session)| some_text(session.worktree.as_ref().and_then(|worktree| worktree.branch.as_ref())))
    };
    let mut out: Vec<WorktreeFacts> = records
        .iter()
        .map(|record| WorktreeFacts {
            id: record.id.clone(),
            branch: branch_of(&record.id).or_else(|| some_text(record.merge_source_ref.as_ref())),
            merged_at: some_text(record.merged_at.as_ref()),
            deleted: record.deleted_at.as_deref().is_some_and(|at| !at.is_empty()),
        })
        .collect();
    for session in sessions {
        let Some(worktree) = session.worktree.as_ref() else {
            continue;
        };
        let Some(id) = some_text(worktree.id.as_ref()) else {
            continue;
        };
        if out.iter().any(|known| known.id == id) {
            continue;
        }
        out.push(WorktreeFacts {
            branch: branch_of(&id),
            merged_at: some_text(worktree.merged_at.as_ref()),
            deleted: worktree.deleted_at.as_deref().is_some_and(|at| !at.is_empty()),
            id,
        });
    }
    out
}

/// Creators first, then the oldest, then by ID.
fn owner_order(left: &SessionFacts, right: &SessionFacts) -> std::cmp::Ordering {
    right
        .created_worktree
        .cmp(&left.created_worktree)
        .then_with(|| left.created_at.cmp(&right.created_at))
        .then_with(|| left.id.cmp(&right.id))
}

/// The session a worktree belongs to: the one that created it (the oldest, should several say so), else its oldest
/// session; none when no session works there any more.
pub fn owning_session<'a>(worktree: &str, sessions: &'a [SessionFacts]) -> Option<&'a SessionFacts> {
    sessions
        .iter()
        .filter(|session| session.worktree.as_deref() == Some(worktree))
        .min_by(|left, right| owner_order(left, right))
}

/// One repository's input.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RepoInput {
    /// `notes/tasks/<key>.md` (empty when there is none).
    pub list: String,
    /// Worktree notes (`notes/worktrees/`), by worktree record ID.
    pub worktree_notes: Vec<(String, String)>,
    /// Session notes (`notes/sessions/`) of this repository's sessions, by session ID.
    pub session_notes: Vec<(String, String)>,
    /// Line notes that already exist (`notes/lines/`), by line ID: new text is appended to them.
    pub line_notes: Vec<(String, String)>,
    /// The repository's sessions and worktrees.
    pub sessions: Vec<SessionFacts>,
    pub worktrees: Vec<WorktreeFacts>,
}

/// Where a note file lives under `notes/`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NoteDir {
    Lines,
    Sessions,
    Worktrees,
}

impl NoteDir {
    pub fn directory(self) -> &'static str {
        match self {
            NoteDir::Lines => "lines",
            NoteDir::Sessions => "sessions",
            NoteDir::Worktrees => "worktrees",
        }
    }
}

/// A note file to write.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NoteOut {
    pub dir: NoteDir,
    pub id: String,
    pub text: String,
}

/// What the migration did, counted.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MigrationCounts {
    /// Note files read (worktree and session notes).
    pub notes: usize,
    /// Lines a session was put on (from `wt=`).
    pub hooked: usize,
    /// Lines left with nobody on them (no session, a dropped worktree, or a session already on another line).
    pub unhooked: usize,
    /// Lines created for a session or a worktree.
    pub created: usize,
    /// Lines made from note checkboxes.
    pub steps: usize,
    /// Line notes written.
    pub line_notes: usize,
    /// Open lines of merged work moved to the top level.
    pub to_backlog: usize,
    /// `↗` lines dropped.
    pub links_dropped: usize,
}

impl MigrationCounts {
    pub fn add(&mut self, other: &MigrationCounts) {
        self.notes += other.notes;
        self.hooked += other.hooked;
        self.unhooked += other.unhooked;
        self.created += other.created;
        self.steps += other.steps;
        self.line_notes += other.line_notes;
        self.to_backlog += other.to_backlog;
        self.links_dropped += other.links_dropped;
    }
}

impl fmt::Display for MigrationCounts {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} note(s) read, {} line(s) put on a session, {} unhooked, {} created, {} step(s) from notes, {} line \
             note(s), {} open line(s) of merged work to the backlog, {} ↗ link(s) dropped",
            self.notes,
            self.hooked,
            self.unhooked,
            self.created,
            self.steps,
            self.line_notes,
            self.to_backlog,
            self.links_dropped
        )
    }
}

/// One repository's result.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RepoOutput {
    /// The new list.
    pub list: String,
    /// Note files to write (line notes; whole files, existing text included).
    pub files: Vec<NoteOut>,
    /// The input notes now carried by the list and line notes: their files go to the backup.
    pub retired: Vec<(NoteDir, String)>,
    /// What it did, a sentence a line.
    pub report: Vec<String>,
    pub counts: MigrationCounts,
}

// ── Notes, read as steps and prose ───────────────────────────────────────────────────────────────────────────────

/// A checkbox of a note.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Step {
    title: String,
    done: bool,
    /// Its line note: the checkbox's text when the title can't hold it as written, then the text indented under it.
    note: String,
    steps: Vec<Step>,
}

/// A note, split.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct SplitNote {
    steps: Vec<Step>,
    prose: String,
    /// The titles of its `↗` lines.
    links: Vec<String>,
}

impl SplitNote {
    fn is_empty(&self) -> bool {
        self.steps.is_empty() && self.prose.is_empty()
    }
}

const TAB_STOP: usize = 4;

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

fn is_blank(line: &str) -> bool {
    jsstr::js_trim(line).is_empty()
}

/// The lines without blank lines at either end, runs of blank lines as one, and a final newline (empty: nothing).
fn tidy(lines: &[String]) -> String {
    let mut out: Vec<&str> = Vec::new();
    for line in lines {
        let blank = is_blank(line);
        if blank && out.last().is_none_or(|last| last.is_empty()) {
            continue;
        }
        out.push(if blank { "" } else { line.as_str() });
    }
    while out.last().is_some_and(|last| last.is_empty()) {
        out.pop();
    }
    out.join("\n")
}

/// The lines with their common indent removed (blank lines don't count).
fn dedent(lines: &[String]) -> Vec<String> {
    let cut = lines
        .iter()
        .filter(|line| !is_blank(line))
        .map(|line| line.len() - line.trim_start_matches([' ', '\t']).len())
        .min()
        .unwrap_or(0);
    lines
        .iter()
        .map(|line| {
            let lead = line.len() - line.trim_start_matches([' ', '\t']).len();
            line[cut.min(lead)..].to_owned()
        })
        .collect()
}

/// Where a step sits while the note is read: its column and the path of indexes to it.
struct Open {
    col: usize,
    path: Vec<usize>,
}

fn step_at<'a>(steps: &'a mut [Step], path: &[usize]) -> Option<&'a mut Step> {
    let (first, rest) = path.split_first()?;
    let step = steps.get_mut(*first)?;
    if rest.is_empty() {
        Some(step)
    } else {
        step_at(&mut step.steps, rest)
    }
}

/// Splits a note into steps and prose. A checkbox is a step, under the nearest step above it that is indented less
/// (blank lines don't end that); text indented under a step, with no blank line between, is its note; everything else
/// (headings, paragraphs, other bullets, and indented text after a blank line) is prose, and an unindented line of
/// prose ends the nesting.
fn split_note(text: &str) -> SplitNote {
    let text = normalize_newlines(text);
    let mut steps: Vec<Step> = Vec::new();
    // Each step's note, by path: the checkbox's text as written when the title can't hold it, then the lines under it
    // (dedented as a block at the end).
    let mut step_lines: Vec<(Vec<usize>, Option<String>, Vec<String>)> = Vec::new();
    let mut prose: Vec<String> = Vec::new();
    let mut links = Vec::new();
    let mut open: Vec<Open> = Vec::new();
    // Text indented under the last step goes into its note until a blank line.
    let mut attach = false;
    for line in text.split('\n') {
        if let Some(link) = parse_note_task_link(line) {
            links.push(link.title);
            continue;
        }
        if is_blank(line) {
            attach = false;
            prose.push(String::new());
            continue;
        }
        let col = indent_col(line);
        let item = parse_checklist_line(line).and_then(|item| {
            let written = jsstr::js_trim(&item.text).to_owned();
            let text = jsstr::js_trim(&without_task_meta(&item.text)).to_owned();
            // Cut to the limit, it may end in a space the list wouldn't read back.
            let title = jsstr::js_trim(&clean_task_title(&text)).to_owned();
            (!title.is_empty()).then_some((item.checked, title, text, written))
        });
        if let Some((done, title, text, written)) = item {
            while open.last().is_some_and(|top| top.col >= col) {
                open.pop();
            }
            let step = Step {
                title,
                done,
                ..Step::default()
            };
            // Cut or cleaned: its note keeps the checkbox's text as written.
            let head = (jsstr::js_collapse_whitespace(&text) != step.title).then_some(written);
            let path = match open.last() {
                Some(parent) => step_at(&mut steps, &parent.path).map(|owner| {
                    owner.steps.push(step.clone());
                    let mut path = parent.path.clone();
                    path.push(owner.steps.len() - 1);
                    path
                }),
                None => {
                    steps.push(step.clone());
                    Some(vec![steps.len() - 1])
                }
            };
            if let Some(path) = path {
                if head.is_some() {
                    step_lines.push((path.clone(), head, Vec::new()));
                }
                open.push(Open { col, path });
                attach = true;
                continue;
            }
        }
        if attach && col > 0 {
            while open.last().is_some_and(|top| top.col >= col) {
                open.pop();
            }
            if let Some(owner) = open.last() {
                match step_lines.iter_mut().find(|(path, _, _)| *path == owner.path) {
                    Some((_, _, lines)) => lines.push(line.to_owned()),
                    None => step_lines.push((owner.path.clone(), None, vec![line.to_owned()])),
                }
                continue;
            }
        }
        if col == 0 {
            open.clear();
        }
        attach = false;
        prose.push(line.to_owned());
    }
    for (path, head, lines) in step_lines {
        if let Some(step) = step_at(&mut steps, &path) {
            let mut all: Vec<String> = head.into_iter().collect();
            all.extend(dedent(&lines));
            step.note = tidy(&all);
        }
    }
    SplitNote {
        steps,
        prose: tidy(&prose),
        links,
    }
}

// ── The migration ────────────────────────────────────────────────────────────────────────────────────────────────

fn quoted(title: &str) -> String {
    format!("\u{201c}{title}\u{201d}")
}

/// What a note is attached to.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Source {
    Worktree(String),
    Session(String),
}

/// The line a session gets when something needs one, before it is created.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Needed {
    Session(String),
    /// A worktree with no session left: titled after its branch.
    Worktree(String),
}

struct Migration<'a, 'b> {
    editor: Editor<'b>,
    input: &'a RepoInput,
    sessions: HashMap<&'a str, &'a SessionFacts>,
    worktrees: HashMap<&'a str, &'a WorktreeFacts>,
    /// The line each session is on.
    line_of: HashMap<String, usize>,
    /// Lines created here, by what they were created for (a session's may be unhooked: its worktree was dropped).
    created_for_session: HashMap<String, usize>,
    created_for_worktree: HashMap<String, usize>,
    /// The first line that had `wt=` for each worktree.
    worktree_line: HashMap<String, usize>,
    /// Lines whose work was merged, with the worktree.
    merged: Vec<(usize, String)>,
    /// Lines created here for merged work.
    created_merged: Vec<(usize, String)>,
    /// Lines created here, with the index of the report line that says so.
    created: Vec<(usize, usize)>,
    /// The notes now carried by the list and line notes.
    retired: Vec<(NoteDir, String)>,
    /// Each line's note, in pieces (appended with a blank line between).
    notes: Vec<(usize, Vec<String>)>,
    report: Vec<String>,
    counts: MigrationCounts,
}

impl Migration<'_, '_> {
    fn worktree(&self, id: &str) -> WorktreeFacts {
        self.worktrees.get(id).map_or_else(
            || WorktreeFacts {
                id: id.to_owned(),
                ..WorktreeFacts::default()
            },
            |facts| (*facts).clone(),
        )
    }

    fn owner(&self, worktree: &str) -> Option<&SessionFacts> {
        owning_session(worktree, &self.input.sessions)
    }

    fn line_label(&self, serial: usize) -> String {
        match self.editor.node(serial) {
            Ok(node) => format!("{} [{}]", quoted(&node.title), node.meta.t.as_deref().unwrap_or("")),
            Err(_) => "a line".to_owned(),
        }
    }

    fn session_label(&self, id: &str) -> String {
        match self.sessions.get(id) {
            Some(session) => format!("session {} [{id}]", quoted(&session.title)),
            None => format!("session [{id}]"),
        }
    }

    fn worktree_label(&self, id: &str) -> String {
        match self.worktree(id).branch {
            Some(branch) => format!("worktree ⎇ {branch} [{id}]"),
            None => format!("worktree [{id}]"),
        }
    }

    fn add_note(&mut self, serial: usize, text: String) {
        if text.is_empty() {
            return;
        }
        match self.notes.iter_mut().find(|(at, _)| *at == serial) {
            Some((_, pieces)) => pieces.push(text),
            None => self.notes.push((serial, vec![text])),
        }
    }

    /// Step 1: who is on each line.
    fn seat(&mut self) -> Result<()> {
        for (serial, _) in self.editor.lines() {
            let node = self.editor.node(serial)?;
            let s = node.meta.s.clone().filter(|s| !s.is_empty());
            let wt = node.meta.wt.clone().filter(|wt| !wt.is_empty());
            let done = node.done;
            if s.is_none() && wt.is_none() {
                continue;
            }
            if let Some(wt) = &wt {
                self.worktree_line.entry(wt.clone()).or_insert(serial);
                if self.worktree(wt).merged_at.is_some() {
                    self.merged.push((serial, wt.clone()));
                }
            }
            let label = self.line_label(serial);
            let who = match (s, wt) {
                (Some(s), _) => s,
                (None, Some(wt)) => {
                    let facts = self.worktree(&wt);
                    if facts.dropped() {
                        let node = self.editor.node_mut(serial)?;
                        node.meta.wt = None;
                        if !done && node.meta.tried.is_none() {
                            node.meta.tried.clone_from(&facts.branch);
                        }
                        self.counts.unhooked += 1;
                        self.report.push(format!(
                            "{label}: nobody on it, its {} was deleted unmerged",
                            self.worktree_label(&wt)
                        ));
                        continue;
                    }
                    match self.owner(&wt).map(|owner| owner.id.clone()) {
                        Some(owner) => owner,
                        None => {
                            self.editor.node_mut(serial)?.meta.wt = None;
                            self.counts.unhooked += 1;
                            self.report.push(format!(
                                "{label}: nobody on it, no session works in its {} any more",
                                self.worktree_label(&wt)
                            ));
                            continue;
                        }
                    }
                }
                (None, None) => continue,
            };
            if let Some(&first) = self.line_of.get(&who) {
                let node = self.editor.node_mut(serial)?;
                node.meta.s = None;
                node.meta.wt = None;
                self.counts.unhooked += 1;
                self.report.push(format!(
                    "{label}: nobody on it, {} is on {} already",
                    self.session_label(&who),
                    self.line_label(first)
                ));
                continue;
            }
            self.line_of.insert(who.clone(), serial);
            let node = self.editor.node_mut(serial)?;
            if let Some(wt) = node.meta.wt.take() {
                node.meta.s = Some(who.clone());
                self.counts.hooked += 1;
                self.report.push(format!(
                    "{label}: {} is on it (was its {})",
                    self.session_label(&who),
                    self.worktree_label(&wt)
                ));
            }
        }
        Ok(())
    }

    /// Step 2: details leave the list for the line notes.
    fn details(&mut self) -> Result<()> {
        let mut moved = Vec::new();
        for (serial, _) in self.editor.lines() {
            let body = self.editor.take_details(serial)?;
            let lines: Vec<String> = body.split('\n').map(str::to_owned).collect();
            let text = tidy(&lines);
            if !text.is_empty() {
                moved.push(self.line_label(serial));
                self.add_note(serial, text);
            }
        }
        if !moved.is_empty() {
            self.report
                .push(format!("details moved into the line notes of {}", moved.join(", ")));
        }
        Ok(())
    }

    /// The line a note goes to, if it has one already; else what has to be created.
    fn target(&self, source: &Source) -> std::result::Result<usize, Needed> {
        match source {
            Source::Session(id) => self
                .line_of
                .get(id)
                .or_else(|| self.created_for_session.get(id))
                .copied()
                .ok_or_else(|| Needed::Session(id.clone())),
            Source::Worktree(id) => {
                let owner = self.owner(id).map(|owner| owner.id.clone());
                if let Some(owner) = &owner
                    && let Some(&line) = self.line_of.get(owner).or_else(|| self.created_for_session.get(owner))
                {
                    return Ok(line);
                }
                if let Some(&line) = self.worktree_line.get(id).or_else(|| self.created_for_worktree.get(id)) {
                    return Ok(line);
                }
                Err(owner.map_or_else(|| Needed::Worktree(id.clone()), Needed::Session))
            }
        }
    }

    /// How deep a session is in the session tree (0: no parent here).
    fn depth(&self, id: &str) -> usize {
        let mut depth = 0;
        let mut current = self.sessions.get(id).and_then(|session| session.parent.as_deref());
        while let Some(parent) = current {
            depth += 1;
            if depth > self.sessions.len() {
                break;
            }
            current = self.sessions.get(parent).and_then(|session| session.parent.as_deref());
        }
        depth
    }

    /// Creates the lines the notes need: sessions' first (parents before their sub-sessions, then the oldest), then
    /// worktrees with no session left.
    fn create(&mut self, needed: Vec<Needed>) -> Result<()> {
        let mut sessions: Vec<String> = Vec::new();
        let mut worktrees: Vec<String> = Vec::new();
        for need in needed {
            match need {
                Needed::Session(id) if !sessions.contains(&id) => sessions.push(id),
                Needed::Worktree(id) if !worktrees.contains(&id) => worktrees.push(id),
                _ => {}
            }
        }
        sessions.sort_by(|left, right| {
            let created = |id: &str| self.sessions.get(id).map(|session| session.created_at.clone());
            self.depth(left)
                .cmp(&self.depth(right))
                .then_with(|| created(left).cmp(&created(right)))
                .then_with(|| left.cmp(right))
        });
        worktrees.sort();
        for id in sessions {
            let Some(session) = self.sessions.get(id.as_str()).copied() else {
                continue;
            };
            let worktree = session.worktree.as_deref().map(|wt| self.worktree(wt));
            let dropped = worktree.as_ref().is_some_and(WorktreeFacts::dropped);
            let parent = session
                .parent
                .as_deref()
                .and_then(|parent| self.line_of.get(parent).copied());
            let mut title = clean_task_title(&session.title);
            if title.is_empty() {
                title = format!("Session {id}");
            }
            let meta = TaskMeta {
                t: Some(self.editor.new_id()),
                s: (!dropped).then(|| id.clone()),
                tried: worktree.as_ref().filter(|_| dropped).and_then(|wt| wt.branch.clone()),
                ..TaskMeta::default()
            };
            let serial = self.editor.add_line(title, false, meta, parent, None)?;
            self.counts.created += 1;
            self.created_for_session.insert(id.clone(), serial);
            if dropped {
                self.counts.unhooked += 1;
            } else {
                self.line_of.insert(id.clone(), serial);
            }
            if let Some(wt) = worktree.filter(|wt| wt.merged_at.is_some()) {
                self.created_merged.push((serial, wt.id));
            }
            let place = match parent {
                Some(parent) => format!("under {}", self.line_label(parent)),
                None => "at the top level".to_owned(),
            };
            let who = if dropped {
                "nobody on it, its worktree was deleted unmerged".to_owned()
            } else {
                "the session is on it".to_owned()
            };
            self.created.push((serial, self.report.len()));
            self.report.push(format!(
                "created {} {place} for {} ({who})",
                self.line_label(serial),
                self.session_label(&id)
            ));
        }
        for id in worktrees {
            let facts = self.worktree(&id);
            let title = facts
                .branch
                .as_deref()
                .map(clean_task_title)
                .filter(|title| !title.is_empty())
                .unwrap_or_else(|| format!("Worktree {id}"));
            let meta = TaskMeta {
                t: Some(self.editor.new_id()),
                tried: facts.branch.clone().filter(|_| facts.dropped()),
                ..TaskMeta::default()
            };
            let serial = self.editor.add_line(title, false, meta, None, None)?;
            self.counts.created += 1;
            self.counts.unhooked += 1;
            self.created_for_worktree.insert(id.clone(), serial);
            if facts.merged_at.is_some() {
                self.created_merged.push((serial, id.clone()));
            }
            self.created.push((serial, self.report.len()));
            self.report.push(format!(
                "created {} at the top level for its {} (nobody on it: no session works there any more)",
                self.line_label(serial),
                self.worktree_label(&id)
            ));
        }
        Ok(())
    }

    fn add_steps(&mut self, steps: &[Step], parent: usize) -> Result<usize> {
        let mut count = 0;
        for step in steps {
            let meta = TaskMeta {
                t: Some(self.editor.new_id()),
                ..TaskMeta::default()
            };
            let serial = self
                .editor
                .add_line(step.title.clone(), step.done, meta, Some(parent), None)?;
            count += 1;
            self.add_note(serial, step.note.clone());
            count += self.add_steps(&step.steps, serial)?;
        }
        Ok(count)
    }

    fn source_label(&self, source: &Source) -> String {
        match source {
            Source::Worktree(id) => format!("the note of {}", self.worktree_label(id)),
            Source::Session(id) => format!("the note of {}", self.session_label(id)),
        }
    }

    /// Step 3: the notes.
    fn notes(&mut self, notes: Vec<(Source, SplitNote)>) -> Result<()> {
        let needed: Vec<Needed> = notes
            .iter()
            .filter(|(_, note)| !note.is_empty())
            .filter_map(|(source, _)| self.target(source).err())
            .collect();
        self.create(needed)?;
        for (source, note) in notes {
            let label = self.source_label(&source);
            let unknown = matches!(&source, Source::Session(id) if !self.sessions.contains_key(id.as_str()));
            if !unknown || note.is_empty() {
                self.retired.push(match &source {
                    Source::Worktree(id) => (NoteDir::Worktrees, id.clone()),
                    Source::Session(id) => (NoteDir::Sessions, id.clone()),
                });
            }
            if !note.links.is_empty() {
                self.counts.links_dropped += note.links.len();
                let titles: Vec<String> = note.links.iter().map(|title| quoted(title)).collect();
                self.report
                    .push(format!("{label}: dropped its ↗ line(s) {}", titles.join(", ")));
            }
            if note.is_empty() {
                if note.links.is_empty() {
                    self.report.push(format!("{label}: empty, nothing to move"));
                }
                continue;
            }
            let Ok(target) = self.target(&source) else {
                // A session the records don't know: its note stays where it is.
                self.report.push(format!("{label}: left as it is (no such session)"));
                continue;
            };
            let steps = self.add_steps(&note.steps, target)?;
            self.counts.steps += steps;
            let prose = !note.prose.is_empty();
            if prose {
                self.add_note(target, note.prose);
            }
            let mut what = Vec::new();
            if steps > 0 {
                what.push(format!("{steps} line(s) under {}", self.line_label(target)));
            }
            if prose {
                what.push(format!("its prose into the note of {}", self.line_label(target)));
            }
            self.report.push(format!("{label}: {}", what.join("; ")));
        }
        // A sub-session's line, made before its parent's steps were added, goes after them.
        for (serial, _) in self.created.clone() {
            if let Some(parent) = self.editor.parent_of(serial) {
                self.editor.move_last_under(serial, parent)?;
            }
        }
        Ok(())
    }

    /// The top-level line `serial` is in.
    fn top_of(&self, serial: usize) -> usize {
        let mut top = serial;
        while let Some(parent) = self.editor.parent_of(top) {
            top = parent;
        }
        top
    }

    /// Step 4: merged work.
    fn merged(&mut self) -> Result<()> {
        let mut lines: Vec<(usize, String)> = self.merged.clone();
        lines.extend(self.created_merged.iter().cloned());
        for (session, &serial) in &self.line_of {
            if let Some(wt) = self
                .sessions
                .get(session.as_str())
                .and_then(|facts| facts.worktree.clone())
                && self.worktree(&wt).merged_at.is_some()
            {
                lines.push((serial, wt));
            }
        }
        let order: Vec<usize> = self.editor.lines().iter().map(|(serial, _)| *serial).collect();
        lines.sort_by(|(left, left_wt), (right, right_wt)| {
            let at = |serial: &usize| order.iter().position(|known| known == serial);
            at(left).cmp(&at(right)).then_with(|| left_wt.cmp(right_wt))
        });
        lines.dedup_by_key(|(serial, _)| *serial);
        for (serial, wt) in lines {
            let open = self.editor.open_lines_under(serial)?;
            if open.is_empty() {
                continue;
            }
            let branch = self.worktree(&wt).branch;
            let mut after = self.top_of(serial);
            for line in &open {
                self.editor.move_after(*line, after)?;
                if let Some(branch) = &branch {
                    self.editor.node_mut(*line)?.meta.from = Some(branch.clone());
                }
                after = *line;
            }
            self.counts.to_backlog += open.len();
            self.report.push(format!(
                "{} open line(s) under {} moved to the top level: its {} was merged",
                open.len(),
                self.line_label(serial),
                self.worktree_label(&wt)
            ));
        }
        // A line created for merged work whose lines all went: nothing needed it after all.
        let mut gone = Vec::new();
        for (serial, report) in self.created.clone() {
            let noted = self.notes.iter().any(|(at, _)| *at == serial);
            if !noted && self.editor.is_bare(serial)? {
                let label = self.line_label(serial);
                self.editor.remove_line(serial)?;
                self.counts.created -= 1;
                gone.push(serial);
                if let Some(line) = self.report.get_mut(report) {
                    *line = format!("{line}; removed again: merging left nothing under {label}");
                }
            }
        }
        self.created_merged.retain(|(serial, _)| !gone.contains(serial));
        let today = self.editor.today().to_owned();
        for (serial, wt) in self.created_merged.clone() {
            let merged_on = self
                .worktree(&wt)
                .merged_at
                .map(|at| at.chars().take(10).collect::<String>())
                .filter(|day| jsstr::date::parse(day).is_some())
                .unwrap_or_else(|| today.clone());
            let node = self.editor.node_mut(serial)?;
            if !node.done {
                node.done = true;
                node.meta.done = Some(merged_on);
                node.meta.auto = Some(TaskAuto::Merge);
            }
        }
        Ok(())
    }

    fn finish(self) -> Result<RepoOutput> {
        let mut files = Vec::new();
        for (serial, pieces) in &self.notes {
            let node = self.editor.node(*serial)?;
            let Some(id) = node.meta.t.clone() else { continue };
            let existing = self
                .input
                .line_notes
                .iter()
                .find(|(line, _)| *line == id)
                .map(|(_, text)| jsstr::js_trim_end(text).to_owned())
                .filter(|text| !jsstr::js_trim(text).is_empty());
            let text = existing
                .into_iter()
                .chain(pieces.iter().cloned())
                .collect::<Vec<_>>()
                .join("\n\n");
            files.push(NoteOut {
                dir: NoteDir::Lines,
                id,
                text: format!("{text}\n"),
            });
        }
        let mut counts = self.counts;
        counts.line_notes = files.len();
        Ok(RepoOutput {
            list: self.editor.text(),
            files,
            retired: self.retired,
            report: self.report,
            counts,
        })
    }
}

/// Migrates one repository (the rules are in the module docs). `new_id` gives the IDs of lines without one and of the
/// lines created; one already in use is skipped.
pub fn migrate_repository(input: &RepoInput, now_ms: i64, new_id: &mut dyn FnMut() -> String) -> Result<RepoOutput> {
    let mut taken: HashSet<String> = TaskList::parse(&input.list)
        .lines()
        .iter()
        .filter_map(|line| line.meta.t.clone())
        .collect();
    taken.extend(input.line_notes.iter().map(|(id, _)| id.clone()));
    let mut fresh = move || {
        let mut attempt = 0usize;
        loop {
            let id = new_id();
            // A generator that keeps repeating itself still ends.
            let id = if attempt < 64 { id } else { format!("{id}{attempt}") };
            if taken.insert(id.clone()) {
                return id;
            }
            attempt += 1;
        }
    };
    let editor = Editor::open(&input.list, now_ms, &mut fresh)?;
    let mut migration = Migration {
        editor,
        input,
        sessions: input
            .sessions
            .iter()
            .map(|session| (session.id.as_str(), session))
            .collect(),
        worktrees: input
            .worktrees
            .iter()
            .map(|worktree| (worktree.id.as_str(), worktree))
            .collect(),
        line_of: HashMap::new(),
        created_for_session: HashMap::new(),
        created_for_worktree: HashMap::new(),
        worktree_line: HashMap::new(),
        merged: Vec::new(),
        created_merged: Vec::new(),
        created: Vec::new(),
        retired: Vec::new(),
        notes: Vec::new(),
        report: Vec::new(),
        counts: MigrationCounts::default(),
    };
    migration.seat()?;
    migration.details()?;
    let mut notes: Vec<(Source, SplitNote)> = Vec::new();
    for (id, text) in &input.worktree_notes {
        notes.push((Source::Worktree(id.clone()), split_note(text)));
    }
    let mut session_notes: Vec<&(String, String)> = input.session_notes.iter().collect();
    session_notes.sort_by(|(left, _), (right, _)| {
        let created = |id: &str| migration.sessions.get(id).map(|session| session.created_at.clone());
        created(left).cmp(&created(right)).then_with(|| left.cmp(right))
    });
    for (id, text) in session_notes {
        notes.push((Source::Session(id.clone()), split_note(text)));
    }
    migration.counts.notes = notes.len();
    migration.notes(notes)?;
    migration.merged()?;
    migration.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn now() -> i64 {
        jsstr::date::parse("2026-11-14T12:00:00Z").unwrap_or(0)
    }

    fn session(
        id: &str,
        title: &str,
        worktree: Option<&str>,
        created: bool,
        parent: Option<&str>,
        at: &str,
    ) -> SessionFacts {
        SessionFacts {
            id: id.to_owned(),
            title: title.to_owned(),
            worktree: worktree.map(str::to_owned),
            created_worktree: created,
            parent: parent.map(str::to_owned),
            created_at: format!("2026-10-{at}T09:00:00.000Z"),
        }
    }

    fn worktree(id: &str, branch: &str) -> WorktreeFacts {
        WorktreeFacts {
            id: id.to_owned(),
            branch: Some(branch.to_owned()),
            merged_at: None,
            deleted: false,
        }
    }

    fn merged(id: &str, branch: &str) -> WorktreeFacts {
        WorktreeFacts {
            merged_at: Some("2026-11-02T10:00:00.000Z".to_owned()),
            ..worktree(id, branch)
        }
    }

    fn owned(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(id, text)| ((*id).to_owned(), (*text).to_owned()))
            .collect()
    }

    /// The migration, new IDs `n1`, `n2`, ...
    fn run(input: &RepoInput) -> RepoOutput {
        let mut n = 0;
        let mut next = || {
            n += 1;
            format!("n{n}")
        };
        migrate_repository(input, now(), &mut next).unwrap_or_else(|error| panic!("migration failed: {error}"))
    }

    /// Each line as `<depth spaces><title>`, then ` [x]` when ticked and its `s=`, `from=` and `tried=`.
    fn tree(text: &str) -> Vec<String> {
        TaskList::parse(text)
            .lines()
            .iter()
            .map(|line| {
                let mut out = format!("{}{}", "  ".repeat(line.depth), line.title);
                if line.done {
                    out.push_str(" [x]");
                }
                for (key, value) in [
                    ("s", &line.meta.s),
                    ("from", &line.meta.from),
                    ("tried", &line.meta.tried),
                ] {
                    if let Some(value) = value {
                        out.push_str(&format!(" {key}={value}"));
                    }
                }
                assert!(line.meta.wt.is_none(), "wt= left on {}", line.title);
                assert!(line.body.is_empty(), "details left on {}", line.title);
                out
            })
            .collect()
    }

    /// Line notes by the title of their line.
    fn notes(output: &RepoOutput) -> BTreeMap<String, String> {
        let list = TaskList::parse(&output.list);
        output
            .files
            .iter()
            .map(|file| {
                assert_eq!(file.dir, NoteDir::Lines);
                let title = list
                    .get(&file.id)
                    .map_or_else(|| format!("?{}", file.id), |line| line.title.clone());
                (title, file.text.clone())
            })
            .collect()
    }

    /// Words of prose: bookkeeping comments, checkbox markers and bullets aside.
    fn words(text: &str) -> Vec<String> {
        normalize_newlines(text)
            .split('\n')
            .filter(|line| parse_note_task_link(line).is_none())
            .flat_map(|line| {
                let line = without_task_meta(line);
                let text = match parse_checklist_line(&line) {
                    Some(item) => item.text,
                    None => line,
                };
                jsstr::js_split_whitespace(&text)
                    .into_iter()
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    fn count(words: Vec<String>) -> HashMap<String, usize> {
        let mut out = HashMap::new();
        for word in words {
            *out.entry(word).or_insert(0) += 1;
        }
        out
    }

    /// Every word of the input's list and notes is in the output's list and files (or its report), as often.
    fn assert_no_text_lost(input: &RepoInput, output: &RepoOutput) {
        let mut before = words(&input.list);
        for (_, text) in input
            .worktree_notes
            .iter()
            .chain(&input.session_notes)
            .chain(&input.line_notes)
        {
            before.extend(words(text));
        }
        let mut after = words(&output.list);
        for file in &output.files {
            after.extend(words(&file.text));
        }
        // Notes not retired are still where they were.
        for (id, text) in &input.session_notes {
            if !output.retired.contains(&(NoteDir::Sessions, id.clone())) {
                after.extend(words(text));
            }
        }
        // Line notes not rewritten are unchanged.
        for (id, text) in &input.line_notes {
            if !output.files.iter().any(|file| &file.id == id) {
                after.extend(words(text));
            }
        }
        let (before, after) = (count(before), count(after));
        for (word, times) in before {
            let kept = after.get(&word).copied().unwrap_or(0);
            assert!(kept >= times, "{word:?}: {times} in the input, {kept} after");
        }
    }

    fn assert_idempotent(input: &RepoInput, output: &RepoOutput) {
        let again = RepoInput {
            list: output.list.clone(),
            worktree_notes: Vec::new(),
            session_notes: input
                .session_notes
                .iter()
                .filter(|(id, _)| !output.retired.contains(&(NoteDir::Sessions, id.clone())))
                .cloned()
                .collect(),
            line_notes: output
                .files
                .iter()
                .map(|file| (file.id.clone(), file.text.clone()))
                .collect(),
            sessions: input.sessions.clone(),
            worktrees: input.worktrees.clone(),
        };
        let second = run(&again);
        assert_eq!(second.list, output.list, "the list changed on a second run");
        assert!(second.files.is_empty(), "a second run wrote {:?}", second.files);
        assert_eq!(second.counts.hooked + second.counts.created + second.counts.steps, 0);
    }

    /// Runs it and checks the properties every run has.
    fn migrate(input: &RepoInput) -> RepoOutput {
        let output = run(input);
        assert_no_text_lost(input, &output);
        assert_idempotent(input, &output);
        assert_eq!(run(input), output, "the same input gave a different result");
        output
    }

    #[test]
    fn a_worktree_task_moves_onto_the_session_that_created_the_worktree() {
        let input = RepoInput {
            list: "- [ ] Login <!-- dh:t=a1 wt=w1 added=2026-10-01 -->\n".to_owned(),
            sessions: vec![
                // Older, but it only joined the worktree (selected it).
                session("joined", "Joined", Some("w1"), false, None, "01"),
                session("maker", "Maker", Some("w1"), true, None, "02"),
            ],
            worktrees: vec![worktree("w1", "login")],
            ..RepoInput::default()
        };
        let output = migrate(&input);
        assert_eq!(output.list, "- [ ] Login <!-- dh:t=a1 s=maker added=2026-10-01 -->\n");
        assert_eq!(output.counts.hooked, 1);
        assert!(
            output.report[0].contains("session \u{201c}Maker\u{201d} [maker] is on it"),
            "{:?}",
            output.report
        );
    }

    #[test]
    fn without_a_creator_the_oldest_session_of_the_worktree_takes_the_task() {
        let input = RepoInput {
            list: "- [ ] Attached work <!-- dh:t=a1 wt=w1 -->\n".to_owned(),
            sessions: vec![
                session("young", "Young", Some("w1"), false, None, "05"),
                session("old", "Old", Some("w1"), false, None, "02"),
                session("elsewhere", "Elsewhere", None, false, None, "01"),
            ],
            worktrees: vec![worktree("w1", "attached")],
            ..RepoInput::default()
        };
        assert_eq!(tree(&migrate(&input).list), ["Attached work s=old"]);
    }

    #[test]
    fn a_worktree_task_with_no_session_left_is_unhooked_and_reported() {
        let input = RepoInput {
            list: "- [ ] Orphan <!-- dh:t=a1 wt=gone -->\n".to_owned(),
            worktrees: vec![worktree("gone", "orphan-branch")],
            ..RepoInput::default()
        };
        let output = migrate(&input);
        assert_eq!(tree(&output.list), ["Orphan"]);
        assert_eq!(output.counts.unhooked, 1);
        assert_eq!(
            output.report,
            [
                "\u{201c}Orphan\u{201d} [a1]: nobody on it, no session works in its worktree ⎇ orphan-branch [gone] any more"
            ]
        );
    }

    #[test]
    fn a_worktree_deleted_unmerged_leaves_its_task_unhooked_and_tried() {
        let input = RepoInput {
            list: "- [ ] Dropped <!-- dh:t=a1 wt=w1 -->\n- [x] Done dropped <!-- dh:t=a2 wt=w1 done=2026-10-01 -->\n"
                .to_owned(),
            worktree_notes: owned(&[("w1", "- [ ] a step\n")]),
            sessions: vec![session("s1", "Dropped work", Some("w1"), true, None, "01")],
            worktrees: vec![WorktreeFacts {
                deleted: true,
                ..worktree("w1", "dropped-branch")
            }],
            ..RepoInput::default()
        };
        let output = migrate(&input);
        // The note's steps go under the task line all the same; only an open line is marked tried.
        assert_eq!(
            tree(&output.list),
            ["Dropped tried=dropped-branch", "  a step", "Done dropped [x]"]
        );
    }

    #[test]
    fn details_become_the_start_of_the_line_note() {
        let input = RepoInput {
            list: "\
- [ ] Port storage <!-- dh:t=a1 -->
  Details of the storage port
  \n  second paragraph
  - [ ] a nested line <!-- dh:t=a2 -->
    its own detail
- [ ] Plain <!-- dh:t=a3 -->
"
            .to_owned(),
            ..RepoInput::default()
        };
        let output = migrate(&input);
        assert_eq!(
            output.list,
            "- [ ] Port storage <!-- dh:t=a1 -->\n  - [ ] a nested line <!-- dh:t=a2 -->\n- [ ] Plain <!-- dh:t=a3 -->\n"
        );
        assert_eq!(
            notes(&output),
            BTreeMap::from([
                (
                    "Port storage".to_owned(),
                    "Details of the storage port\n\nsecond paragraph\n".to_owned()
                ),
                ("a nested line".to_owned(), "its own detail\n".to_owned()),
            ])
        );
    }

    #[test]
    fn worktree_note_checkboxes_become_lines_under_its_task_with_ticks_and_nesting() {
        let input = RepoInput {
            list: "- [ ] Feature <!-- dh:t=a1 wt=w1 -->\n  why: users asked\n".to_owned(),
            worktree_notes: owned(&[(
                "w1",
                "\
# Plan
- [x] schema
- [ ] encryption
  - [ ] key rotation
    rotate monthly
  - [x] at rest
- ↗ Sent away <!-- dh:t=a9 -->

Some thinking about it.
",
            )]),
            sessions: vec![session("s1", "Feature", Some("w1"), true, None, "01")],
            worktrees: vec![worktree("w1", "feature")],
            ..RepoInput::default()
        };
        let output = migrate(&input);
        assert_eq!(
            tree(&output.list),
            [
                "Feature s=s1",
                "  schema [x]",
                "  encryption",
                "    key rotation",
                "    at rest [x]"
            ]
        );
        // Prose after the details, a blank line between; the text under a checkbox is its line's note.
        assert_eq!(
            notes(&output),
            BTreeMap::from([
                (
                    "Feature".to_owned(),
                    "why: users asked\n\n# Plan\n\nSome thinking about it.\n".to_owned()
                ),
                ("key rotation".to_owned(), "rotate monthly\n".to_owned()),
            ])
        );
        assert_eq!(output.retired, [(NoteDir::Worktrees, "w1".to_owned())]);
        assert_eq!(output.counts.steps, 4);
        assert_eq!(output.counts.links_dropped, 1);
        assert!(
            output
                .report
                .iter()
                .any(|line| line.contains("dropped its ↗ line(s) \u{201c}Sent away\u{201d}")),
            "{:?}",
            output.report
        );
    }

    #[test]
    fn a_worktree_note_without_a_task_gets_a_line_for_its_owning_session() {
        let input = RepoInput {
            list: "# Tasks\n\n- [ ] Parent's task <!-- dh:t=a1 s=parent -->\n- [x] Old <!-- dh:t=a2 done=2026-01-01 -->\n\nTrailing prose.\n"
                .to_owned(),
            worktree_notes: owned(&[("w1", "- [ ] child step\n"), ("w2", "only prose here\n")]),
            sessions: vec![
                session("parent", "Parent", None, false, None, "01"),
                session("child", "Child work", Some("w1"), true, Some("parent"), "02"),
                session("lone", "Lone work", Some("w2"), true, None, "03"),
            ],
            worktrees: vec![worktree("w1", "child"), worktree("w2", "lone")],
            ..RepoInput::default()
        };
        let output = migrate(&input);
        // Under its parent session's line, or at the backlog's end.
        assert_eq!(
            output.list,
            "\
# Tasks

- [ ] Parent's task <!-- dh:t=a1 s=parent -->
  - [ ] Child work <!-- dh:t=n2 s=child -->
    - [ ] child step <!-- dh:t=n3 -->
- [ ] Lone work <!-- dh:t=n1 s=lone -->
- [x] Old <!-- dh:t=a2 done=2026-01-01 -->

Trailing prose.
"
        );
        assert_eq!(
            notes(&output),
            BTreeMap::from([("Lone work".to_owned(), "only prose here\n".to_owned())])
        );
        assert_eq!(output.counts.created, 2);
    }

    #[test]
    fn a_worktree_note_with_no_session_left_gets_a_line_named_after_its_branch() {
        let input = RepoInput {
            worktree_notes: owned(&[("w1", "- [ ] leftover\n")]),
            worktrees: vec![worktree("w1", "old-idea")],
            ..RepoInput::default()
        };
        let output = migrate(&input);
        assert_eq!(tree(&output.list), ["old-idea", "  leftover"]);
    }

    #[test]
    fn a_main_checkout_sessions_note_goes_under_its_line_made_if_needed() {
        let input = RepoInput {
            list: "- [ ] Refactor <!-- dh:t=a1 s=m1 -->\n".to_owned(),
            session_notes: owned(&[
                (
                    "m1",
                    "# Daemon\n- [ ] split start()\n- [x] read it\n  - [ ] nested under a done one\n",
                ),
                ("m2", "- [ ] check the tests\n"),
            ]),
            sessions: vec![
                session("m1", "Refactor the daemon", None, false, None, "01"),
                session("m2", "Exited one", None, false, None, "02"),
            ],
            ..RepoInput::default()
        };
        let output = migrate(&input);
        assert_eq!(
            tree(&output.list),
            [
                "Refactor s=m1",
                "  split start()",
                "  read it [x]",
                "    nested under a done one",
                "Exited one s=m2",
                "  check the tests"
            ]
        );
        assert_eq!(
            notes(&output),
            BTreeMap::from([("Refactor".to_owned(), "# Daemon\n".to_owned())])
        );
        assert_eq!(
            output.retired,
            [
                (NoteDir::Sessions, "m1".to_owned()),
                (NoteDir::Sessions, "m2".to_owned())
            ]
        );
    }

    #[test]
    fn open_lines_under_merged_work_move_to_the_backlog_marked_with_its_branch() {
        let input = RepoInput {
            list: "\
- [x] Merged task <!-- dh:t=a1 wt=w1 done=2026-11-02 auto=merge -->
- [ ] Next <!-- dh:t=a2 -->
"
            .to_owned(),
            worktree_notes: owned(&[
                (
                    "w1",
                    "- [x] done step\n- [ ] open one\n  - [ ] under it\n- [ ] open two\n",
                ),
                ("w2", "- [ ] only open\n"),
                ("w3", "- [ ] open\n- [x] closed\nnotes\n"),
            ]),
            sessions: vec![
                session("s1", "Merged", Some("w1"), true, None, "01"),
                session("s2", "Also merged", Some("w2"), true, None, "02"),
                session("s3", "Third", Some("w3"), true, None, "03"),
            ],
            worktrees: vec![
                merged("w1", "feat-one"),
                merged("w2", "feat-two"),
                merged("w3", "feat-three"),
            ],
            ..RepoInput::default()
        };
        let output = migrate(&input);
        assert_eq!(
            tree(&output.list),
            [
                "Merged task [x] s=s1",
                "  done step [x]",
                "open one from=feat-one",
                "  under it",
                "open two from=feat-one",
                "Next",
                // Nothing was left for Also merged's line (made at the backlog's end): it isn't there.
                "only open from=feat-two",
                // Made for merged work, it is ticked as the merge ticks it.
                "Third [x] s=s3",
                "  closed [x]",
                "open from=feat-three",
            ]
        );
        let list = TaskList::parse(&output.list);
        let third = list.lines().iter().find(|line| line.title == "Third").cloned();
        assert_eq!(third.and_then(|line| line.meta.done), Some("2026-11-02".to_owned()));
        assert_eq!(output.counts.to_backlog, 4);
        assert_eq!(output.counts.created, 1);
        assert!(!output.list.contains("s=s2"));
    }

    #[test]
    fn a_session_on_two_lines_stays_on_the_first() {
        let input = RepoInput {
            list: "\
- [ ] First <!-- dh:t=a1 s=s1 -->
- [ ] Second <!-- dh:t=a2 s=s1 -->
- [ ] Its worktree's <!-- dh:t=a3 wt=w1 -->
"
            .to_owned(),
            sessions: vec![session("s1", "One", Some("w1"), true, None, "01")],
            worktrees: vec![worktree("w1", "one")],
            ..RepoInput::default()
        };
        let output = migrate(&input);
        assert_eq!(tree(&output.list), ["First s=s1", "Second", "Its worktree's"]);
        assert_eq!(output.counts.unhooked, 2);
    }

    #[test]
    fn empty_notes_make_nothing_and_no_session_without_content_gets_a_line() {
        let input = RepoInput {
            list: String::new(),
            worktree_notes: owned(&[("w1", "\n  \n"), ("w2", "- ↗ Elsewhere <!-- dh:t=x1 -->\n")]),
            session_notes: owned(&[("m1", "")]),
            sessions: vec![
                session("m1", "Main", None, false, None, "01"),
                session("s1", "Worktree one", Some("w1"), true, None, "02"),
                session("s2", "Worktree two", Some("w2"), true, None, "03"),
                session("quiet", "Nothing to say", None, false, None, "04"),
            ],
            worktrees: vec![worktree("w1", "one"), worktree("w2", "two")],
            ..RepoInput::default()
        };
        let output = migrate(&input);
        assert_eq!(output.list, "");
        assert!(output.files.is_empty());
        assert_eq!(output.counts.created, 0);
        // The empty files are retired all the same: nothing reads them any more.
        assert_eq!(output.retired.len(), 3);
    }

    #[test]
    fn a_checkbox_too_long_for_a_title_keeps_its_text_in_its_note() {
        let long = "word ".repeat(80);
        let input = RepoInput {
            session_notes: owned(&[("m1", &format!("- [ ] {long}\n"))]),
            sessions: vec![session("m1", "Main", None, false, None, "01")],
            ..RepoInput::default()
        };
        let output = migrate(&input);
        let step = TaskList::parse(&output.list).lines().get(1).cloned();
        assert!(step.is_some_and(|line| line.title.starts_with("word word") && jsstr::js_len(&line.title) < 300));
        assert_eq!(
            notes(&output).values().next().map(String::as_str),
            Some(format!("{}\n", long.trim_end()).as_str())
        );
    }

    #[test]
    fn a_line_note_that_exists_already_is_kept_and_appended_to() {
        let input = RepoInput {
            list: "- [ ] Task <!-- dh:t=a1 -->\n  detail\n".to_owned(),
            line_notes: owned(&[("a1", "written before\n")]),
            ..RepoInput::default()
        };
        let output = migrate(&input);
        assert_eq!(
            notes(&output),
            BTreeMap::from([("Task".to_owned(), "written before\n\ndetail\n".to_owned())])
        );
    }

    #[test]
    fn a_note_of_a_session_the_records_dont_know_stays_where_it_is() {
        let input = RepoInput {
            session_notes: owned(&[("ghost", "- [ ] keep me\n")]),
            ..RepoInput::default()
        };
        let output = migrate(&input);
        assert_eq!(output.list, "");
        assert!(output.retired.is_empty());
        assert!(output.report[0].contains("left as it is"), "{:?}", output.report);
    }

    #[test]
    fn new_ids_come_from_the_generator_and_skip_ones_in_use() {
        let input = RepoInput {
            list: "- [ ] Has one <!-- dh:t=n1 -->\n- [ ] Has none\n".to_owned(),
            ..RepoInput::default()
        };
        let output = migrate(&input);
        assert_eq!(
            output.list,
            "- [ ] Has one <!-- dh:t=n1 -->\n- [ ] Has none <!-- dh:t=n2 -->\n"
        );
    }

    // ── A realistic layout (anonymised; the shape of the data the migration was decided against) ──

    /// `count` sessions working in worktree `wt`: the first created it, the rest are its sub-sessions.
    fn worktree_sessions(prefix: &str, title: &str, wt: &str, count: usize, day: &str) -> Vec<SessionFacts> {
        let first = format!("{prefix}-01");
        (1..=count)
            .map(|n| {
                let id = format!("{prefix}-{n:02}");
                if n == 1 {
                    session(&id, title, Some(wt), true, None, day)
                } else {
                    let mut sub = session(&id, &format!("{title} part {n}"), Some(wt), false, Some(&first), day);
                    sub.created_at = format!("2026-10-{day}T10:{n:02}:00.000Z");
                    sub
                }
            })
            .collect()
    }

    fn repo_a() -> RepoInput {
        let mut sessions = Vec::new();
        sessions.extend(worktree_sessions("auth", "Auth rework", "w-auth", 16, "01"));
        sessions.extend(worktree_sessions("sync", "Sync engine", "w-sync", 12, "02"));
        sessions.extend(worktree_sessions("search", "Search index", "w-search", 1, "03"));
        sessions.extend(worktree_sessions("cache", "Cache fix", "w-cache", 1, "04"));
        sessions.extend(worktree_sessions("docs", "Docs pass", "w-docs", 1, "05"));
        sessions.push(session("main-1", "Release prep", None, false, None, "06"));
        sessions.push(session("main-2", "Changelog", None, false, Some("main-1"), "07"));
        sessions.push(session("main-3", "Exited triage", None, false, None, "08"));
        sessions.push(session("main-4", "Flaky tests", None, false, None, "09"));
        for n in 1..=20 {
            sessions.push(session(
                &format!("quiet-{n:02}"),
                &format!("Quick look {n}"),
                None,
                false,
                None,
                "10",
            ));
        }
        RepoInput {
            list: "\
# Tasks

- [ ] Auth rework <!-- dh:t=a0000001 wt=w-auth added=2026-10-01 -->
  Tokens must not reach the browser.
  See the threat model doc.
- [ ] Sync engine <!-- dh:t=a0000002 wt=w-sync added=2026-10-02 -->
- [x] Search index <!-- dh:t=a0000003 wt=w-search done=2026-11-02 auto=merge added=2026-10-03 -->
- [ ] Release prep <!-- dh:t=a0000004 s=main-1 added=2026-10-06 -->
- [ ] Rate limits <!-- dh:t=a0000005 added=2026-10-07 -->
  Per user, not per IP.
  Ask about the burst size.
- [ ] Dark mode <!-- dh:t=a0000006 added=2026-10-07 -->
- [ ] Export to CSV <!-- dh:t=a0000007 added=2026-10-08 -->
- [ ] Audit log <!-- dh:t=a0000008 added=2026-10-08 -->
- [ ] Onboarding copy <!-- dh:t=a0000009 added=2026-10-09 -->
- [x] Fix the build <!-- dh:t=a000000a done=2026-10-02 added=2026-10-01 -->
  It was the lockfile.
- [x] Upgrade the runtime <!-- dh:t=a000000b done=2026-10-04 added=2026-10-01 -->
- [x] Drop the old API <!-- dh:t=a000000c done=2026-10-05 added=2026-10-01 -->
- [ ] Billing page <!-- dh:t=a000000d added=2026-10-10 -->
"
            .to_owned(),
            worktree_notes: owned(&[
                (
                    "w-auth",
                    "\
## Approach
- [ ] move tokens server-side
- [ ] session cookie flags
- [ ] refresh endpoint
- [ ] revoke on logout
- [ ] tests for expiry
Keep the old flow behind a flag until the release.
",
                ),
                (
                    "w-sync",
                    "\
Conflicts resolve last-writer-wins for now.
- [ ] queue offline edits
- [ ] retry with backoff
- [ ] dedupe by op id
- [ ] tombstones
- [ ] compaction
- [ ] metrics
- [ ] docs
Revisit CRDTs later.
",
                ),
                ("w-search", "- [ ] reindex job\n- [ ] ranking tweaks\n"),
                ("w-cache", "- [ ] invalidate on deploy\n"),
                ("w-docs", "- [ ] screenshots\n"),
            ]),
            session_notes: owned(&[
                (
                    "main-1",
                    "- [ ] version bump\n- [ ] changelog\n- [ ] tag\n- [ ] notes\n- [ ] announce\n- [ ] close milestone\n",
                ),
                ("main-2", "- [ ] group by area\n"),
                ("main-3", "# Triage\n- [x] label the backlog\n"),
                ("main-4", "- [ ] quarantine the flaky ones\n- [ ] file issues\n"),
            ]),
            line_notes: Vec::new(),
            sessions,
            worktrees: vec![
                worktree("w-auth", "auth-rework"),
                worktree("w-sync", "sync-engine"),
                merged("w-search", "search-index"),
                merged("w-cache", "cache-fix"),
                worktree("w-docs", "docs-pass"),
            ],
        }
    }

    fn repo_b() -> RepoInput {
        let mut sessions = Vec::new();
        sessions.extend(worktree_sessions("api", "API v2", "w-api", 14, "01"));
        sessions.extend(worktree_sessions("perf", "Perf pass", "w-perf", 1, "02"));
        sessions.extend(worktree_sessions("ci", "CI cache", "w-ci", 1, "03"));
        sessions.extend(worktree_sessions("lint", "Lint rules", "w-lint", 1, "04"));
        RepoInput {
            list: "\
- [ ] API v2 <!-- dh:t=b0000001 wt=w-api added=2026-10-01 -->
  Versioned under /v2.
  Old clients keep /v1 for a year.
- [ ] Perf pass <!-- dh:t=b0000002 wt=w-perf added=2026-10-02 -->
- [ ] CI cache <!-- dh:t=b0000003 wt=w-ci added=2026-10-03 -->
"
            .to_owned(),
            worktree_notes: owned(&[
                ("w-api", "- [x] route table\n- [ ] pagination\n"),
                ("w-perf", "Profile the cold start first.\n"),
                ("w-ci", "- [ ] cache the toolchain\n"),
                ("w-lint", "- [ ] ban unwrap\n"),
            ]),
            sessions,
            worktrees: vec![
                worktree("w-api", "api-v2"),
                worktree("w-perf", "perf-pass"),
                worktree("w-ci", "ci-cache"),
                worktree("w-lint", "lint-rules"),
            ],
            ..RepoInput::default()
        }
    }

    #[test]
    fn a_realistic_layout_migrates_as_decided() {
        let a = migrate(&repo_a());
        assert_eq!(
            a.list,
            "\
# Tasks

- [ ] Auth rework <!-- dh:t=a0000001 s=auth-01 added=2026-10-01 -->
  - [ ] move tokens server-side <!-- dh:t=n6 -->
  - [ ] session cookie flags <!-- dh:t=n7 -->
  - [ ] refresh endpoint <!-- dh:t=n8 -->
  - [ ] revoke on logout <!-- dh:t=n9 -->
  - [ ] tests for expiry <!-- dh:t=n10 -->
- [ ] Sync engine <!-- dh:t=a0000002 s=sync-01 added=2026-10-02 -->
  - [ ] queue offline edits <!-- dh:t=n11 -->
  - [ ] retry with backoff <!-- dh:t=n12 -->
  - [ ] dedupe by op id <!-- dh:t=n13 -->
  - [ ] tombstones <!-- dh:t=n14 -->
  - [ ] compaction <!-- dh:t=n15 -->
  - [ ] metrics <!-- dh:t=n16 -->
  - [ ] docs <!-- dh:t=n17 -->
- [x] Search index <!-- dh:t=a0000003 s=search-01 done=2026-11-02 auto=merge added=2026-10-03 -->
- [ ] reindex job <!-- dh:t=n18 from=search-index -->
- [ ] ranking tweaks <!-- dh:t=n19 from=search-index -->
- [ ] Release prep <!-- dh:t=a0000004 s=main-1 added=2026-10-06 -->
  - [ ] version bump <!-- dh:t=n22 -->
  - [ ] changelog <!-- dh:t=n23 -->
  - [ ] tag <!-- dh:t=n24 -->
  - [ ] notes <!-- dh:t=n25 -->
  - [ ] announce <!-- dh:t=n26 -->
  - [ ] close milestone <!-- dh:t=n27 -->
  - [ ] Changelog <!-- dh:t=n5 s=main-2 -->
    - [ ] group by area <!-- dh:t=n28 -->
- [ ] Rate limits <!-- dh:t=a0000005 added=2026-10-07 -->
- [ ] Dark mode <!-- dh:t=a0000006 added=2026-10-07 -->
- [ ] Export to CSV <!-- dh:t=a0000007 added=2026-10-08 -->
- [ ] Audit log <!-- dh:t=a0000008 added=2026-10-08 -->
- [ ] Onboarding copy <!-- dh:t=a0000009 added=2026-10-09 -->
- [x] Fix the build <!-- dh:t=a000000a done=2026-10-02 added=2026-10-01 -->
- [x] Upgrade the runtime <!-- dh:t=a000000b done=2026-10-04 added=2026-10-01 -->
- [x] Drop the old API <!-- dh:t=a000000c done=2026-10-05 added=2026-10-01 -->
- [ ] Billing page <!-- dh:t=a000000d added=2026-10-10 -->
- [ ] invalidate on deploy <!-- dh:t=n20 from=cache-fix -->
- [ ] Docs pass <!-- dh:t=n2 s=docs-01 -->
  - [ ] screenshots <!-- dh:t=n21 -->
- [ ] Exited triage <!-- dh:t=n3 s=main-3 -->
  - [x] label the backlog <!-- dh:t=n29 -->
- [ ] Flaky tests <!-- dh:t=n4 s=main-4 -->
  - [ ] quarantine the flaky ones <!-- dh:t=n30 -->
  - [ ] file issues <!-- dh:t=n31 -->
"
        );
        assert_eq!(
            notes(&a),
            BTreeMap::from([
                (
                    "Auth rework".to_owned(),
                    "Tokens must not reach the browser.\nSee the threat model doc.\n\n## Approach\nKeep the old flow \
                     behind a flag until the release.\n"
                        .to_owned()
                ),
                ("Exited triage".to_owned(), "# Triage\n".to_owned()),
                ("Fix the build".to_owned(), "It was the lockfile.\n".to_owned()),
                (
                    "Rate limits".to_owned(),
                    "Per user, not per IP.\nAsk about the burst size.\n".to_owned()
                ),
                (
                    "Sync engine".to_owned(),
                    "Conflicts resolve last-writer-wins for now.\nRevisit CRDTs later.\n".to_owned()
                ),
            ])
        );
        // Lines only for sessions with something on them: none for the 20 quick sessions or the worktrees' other
        // sessions, and none left for the merged worktree whose only step was open.
        let seated: Vec<String> = TaskList::parse(&a.list)
            .lines()
            .iter()
            .filter_map(|line| line.meta.s.clone())
            .collect();
        assert_eq!(
            seated,
            [
                "auth-01",
                "sync-01",
                "search-01",
                "main-1",
                "main-2",
                "docs-01",
                "main-3",
                "main-4"
            ]
        );
        assert_eq!(a.retired.len(), 9);
        assert_eq!(
            a.counts,
            MigrationCounts {
                notes: 9,
                hooked: 3,
                unhooked: 0,
                created: 4,
                steps: 26,
                line_notes: 5,
                to_backlog: 3,
                links_dropped: 0,
            }
        );

        let b = migrate(&repo_b());
        assert_eq!(
            b.list,
            "\
- [ ] API v2 <!-- dh:t=b0000001 s=api-01 added=2026-10-01 -->
  - [x] route table <!-- dh:t=n2 -->
  - [ ] pagination <!-- dh:t=n3 -->
- [ ] Perf pass <!-- dh:t=b0000002 s=perf-01 added=2026-10-02 -->
- [ ] CI cache <!-- dh:t=b0000003 s=ci-01 added=2026-10-03 -->
  - [ ] cache the toolchain <!-- dh:t=n4 -->
- [ ] Lint rules <!-- dh:t=n1 s=lint-01 -->
  - [ ] ban unwrap <!-- dh:t=n5 -->
"
        );
        assert_eq!(
            notes(&b),
            BTreeMap::from([
                (
                    "API v2".to_owned(),
                    "Versioned under /v2.\nOld clients keep /v1 for a year.\n".to_owned()
                ),
                ("Perf pass".to_owned(), "Profile the cold start first.\n".to_owned()),
            ])
        );
        assert_eq!(b.retired.len(), 4);
    }

    #[test]
    fn owning_session_prefers_the_creator_then_the_oldest() {
        let sessions = vec![
            session("b", "B", Some("w"), false, None, "01"),
            session("c", "C", Some("w"), true, None, "05"),
            session("a", "A", Some("w"), true, None, "03"),
            session("x", "X", Some("other"), true, None, "01"),
        ];
        assert_eq!(owning_session("w", &sessions).map(|s| s.id.as_str()), Some("a"));
        assert_eq!(owning_session("w", &sessions[..1]).map(|s| s.id.as_str()), Some("b"));
        assert_eq!(owning_session("none", &sessions), None);
    }
}
