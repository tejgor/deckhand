//! The Tasks board (b) as pure data: the repository's one list as a tree (`docs/future/work-items.md`). Its rows (the
//! open lines in list order with what is under them, then the finished trees of this week and the folded older ones),
//! who is on a line, where `n` starts a session for one, where `w` can move one, and the Notes view (tab): the
//! scratchpad, the line notes and the session notes, for reading. The tasks flow keeps the selection and the keys.
//!
//! The backlog is not a section of its own: it is the top-level lines nobody is on, in their place on the list.

use std::sync::LazyLock;

use regex::Regex;

use crate::display::{Color, THEME, display_session_title, status_color, status_glyph};
use crate::jsstr::{js_split_whitespace, js_to_lower, js_trim, js_trim_end};
use crate::notes::parse_checklist_line;
use crate::session_order::{SessionLike, sort_sessions_for_sidebar};
use crate::sidebar_model::status_words;
use crate::task_list::{Line, LineCount, MovePlan, MoveTarget, TaskList, plan_move};
use crate::tasks::{group_tasks, link_key};
use crate::tasks_board::{lead_session, urgency, work_sessions};
use crate::work_notes::NoteMap;
use deckhand_protocol::data::{SessionRecord, TaskAuto};

/// A note's heading line: `/^#{1,6}[ \t]+(.*)$/`.
static HEADING: LazyLock<Regex> =
    LazyLock::new(|| Regex::new("^#{1,6}[ \t]+([^\n\r\u{2028}\u{2029}]*)$").expect("HEADING is a valid regex"));

fn truthy(value: Option<&str>) -> bool {
    value.is_some_and(|value| !value.is_empty())
}

fn archived(session: &SessionRecord) -> bool {
    truthy(session.archived_at.as_deref())
}

fn find_session<'a, S: SessionLike>(sessions: &'a [S], id: &str) -> Option<&'a SessionRecord> {
    sessions
        .iter()
        .map(SessionLike::session)
        .find(|session| session.id == id)
}

// ── Who is on a line ─────────────────────────────────────────────────────────────────────────────────────────────

/// What a line's row says about who is on it: the session's state glyph and name, or how it ended, or what it was
/// (tried, left open).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LineState {
    pub glyph: String,
    pub color: Color,
    /// The session on it (its name as the sidebar shows it).
    pub who: Option<String>,
    /// Its state words (`working`, `needs you`), `merged 2026-11-10`, `tried in ⎇ branch`, ...; may be empty.
    pub text: String,
    /// The session on it, when it is listed (`o` selects it).
    pub session_id: Option<String>,
}

/// Who is on `line`, from `sessions` (the repository's).
pub fn line_state<S: SessionLike>(line: &Line, sessions: &[S], spinner_frame: &str) -> LineState {
    let meta = &line.meta;
    let given = |value: &Option<String>| value.clone().filter(|value| !value.is_empty());
    if line.done {
        let how = match meta.auto {
            Some(TaskAuto::Merge) => "merged",
            Some(TaskAuto::Done) => "done (D)",
            Some(TaskAuto::Lines) | None => "done",
        };
        return LineState {
            glyph: "✓".to_owned(),
            color: THEME.success,
            who: None,
            text: match given(&meta.done) {
                Some(date) => format!("{how} {date}"),
                None => how.to_owned(),
            },
            session_id: None,
        };
    }
    // A session (`s=`), or a worktree's sessions (legacy `wt=`: the most urgent).
    let lead: Option<Option<&SessionRecord>> = match link_key(meta) {
        Some(key) if key.starts_with("wt:") => {
            let working = work_sessions(&key, sessions);
            let lead = lead_session(&working).map(|lead| lead.session().id.clone());
            Some(lead.and_then(|id| find_session(sessions, &id)))
        }
        Some(_) => Some(meta.s.as_deref().and_then(|s| find_session(sessions, s))),
        None => None,
    };
    match lead {
        Some(Some(session)) => LineState {
            glyph: status_glyph(session, spinner_frame),
            color: status_color(session),
            who: Some(display_session_title(session, sessions)),
            text: if urgency(session) == 3 {
                "needs you"
            } else {
                status_words(session)
            }
            .to_owned(),
            session_id: Some(session.id.clone()),
        },
        Some(None) => LineState {
            glyph: "◌".to_owned(),
            color: THEME.muted,
            who: None,
            text: "session not shown".to_owned(),
            session_id: None,
        },
        None => LineState {
            glyph: "○".to_owned(),
            color: THEME.muted,
            who: None,
            text: if let Some(tried) = given(&meta.tried) {
                format!("tried in ⎇ {tried}")
            } else if let Some(from) = given(&meta.from) {
                format!("left open in ⎇ {from}")
            } else {
                String::new()
            },
            session_id: None,
        },
    }
}

// ── The board's rows ─────────────────────────────────────────────────────────────────────────────────────────────

/// Where a line's row is: the open lines (in list order) or the finished trees.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Section {
    Open,
    Done,
}

/// A note the Notes view lists.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum NoteOf {
    /// The repository's scratchpad.
    Scratchpad,
    /// A line's note (its ID).
    Line(String),
    /// A session's note (its ID).
    Session(String),
}

impl NoteOf {
    fn key(&self) -> String {
        match self {
            NoteOf::Scratchpad => "scratchpad".to_owned(),
            NoteOf::Line(id) => format!("line:{id}"),
            NoteOf::Session(id) => format!("session:{id}"),
        }
    }
}

/// How a note's line is shown in the Notes view.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NoteLineStyle {
    Open,
    Done,
    Heading,
    Text,
}

/// One row of the board (or of its Notes view).
#[derive(Clone, Debug, PartialEq)]
pub enum BoardRow {
    Heading {
        text: String,
        count: Option<usize>,
    },
    Line {
        line: Box<Line>,
        section: Section,
        /// Its depth as shown (under `v`, below the line shown first).
        depth: usize,
        /// Ticked and total lines directly under it, when it has any.
        count: Option<LineCount>,
        /// Its lines are hidden (`c`).
        folded: bool,
        /// It has a brief (`✎`).
        brief: bool,
    },
    Older {
        count: usize,
        shown: bool,
    },
    /// `adds`: selectable, so `a` adds there.
    Empty {
        text: String,
        adds: bool,
    },
    /// The Notes view: a note's name...
    NoteHead {
        note: NoteOf,
        label: String,
        /// A line note's depth in the list.
        depth: usize,
        /// A line note's line is ticked.
        done: bool,
        brief: bool,
        /// No text yet.
        empty: bool,
    },
    /// ...then each line of it (`line`: 0-based in its text; `depth`: its head's).
    NoteLine {
        note: NoteOf,
        line: usize,
        depth: usize,
        text: String,
        style: NoteLineStyle,
    },
}

/// Whether the selection can rest on `row`.
pub fn selectable_row(row: &BoardRow) -> bool {
    match row {
        BoardRow::Line { .. } | BoardRow::Older { .. } | BoardRow::NoteHead { .. } | BoardRow::NoteLine { .. } => true,
        BoardRow::Empty { adds, .. } => *adds,
        BoardRow::Heading { .. } => false,
    }
}

/// What keeps the selection on its row across updates.
pub fn row_key(row: &BoardRow) -> Option<String> {
    match row {
        BoardRow::Line { line, .. } => Some(format!("line:{}", line.id)),
        BoardRow::Older { .. } => Some("older".to_owned()),
        BoardRow::Empty { adds: true, .. } => Some("empty".to_owned()),
        BoardRow::NoteHead { note, .. } => Some(format!("note:{}", note.key())),
        BoardRow::NoteLine { note, line, .. } => Some(format!("noteline:{}:{line}", note.key())),
        BoardRow::Empty { adds: false, .. } | BoardRow::Heading { .. } => None,
    }
}

/// What the board (or its Notes view) shows: everything, the tree under one line (`v`), or nothing (`v` on a session
/// without a line, or under one).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BoardScope<'a> {
    All,
    Under(&'a str),
    Nothing,
}

/// The line `v` shows for session `session_id`: its own, else the nearest of its ancestors' lines.
pub fn scope_root<'a, S: SessionLike>(list: &'a TaskList, session_id: &str, sessions: &[S]) -> Option<&'a Line> {
    let mut id = Some(session_id.to_owned());
    // Each step goes up a parent; the bound guards a cycle in hand-edited records.
    for _ in 0..=sessions.len() {
        let current = id.take()?;
        if let Some(line) = list.line_of_session(&current) {
            return Some(line);
        }
        id = find_session(sessions, &current)
            .and_then(|session| session.parent_session_id.clone())
            .filter(|parent| !parent.is_empty());
    }
    None
}

/// The board's options.
#[derive(Clone, Copy)]
pub struct BoardOptions<'a> {
    /// The older finished trees are shown (the folded row opened).
    pub show_older: bool,
    pub now_ms: i64,
    pub scope: BoardScope<'a>,
    /// Lines whose lines are hidden (`c`).
    pub folded: &'a [String],
    /// For the brief marker.
    pub notes: &'a NoteMap,
}

/// A line is finished when it and every line under it are ticked.
fn tree_done(list: &TaskList, line: &Line) -> bool {
    list.subtree(&line.id).iter().all(|line| line.done)
}

/// `root` and the lines under it as rows (those under a folded line left out), depths from `base`.
fn push_tree(rows: &mut Vec<BoardRow>, list: &TaskList, root: &Line, section: Section, options: &BoardOptions<'_>) {
    let base = root.depth;
    let mut hidden_below: Option<usize> = None;
    for line in list.subtree(&root.id) {
        if let Some(depth) = hidden_below {
            if line.depth > depth {
                continue;
            }
            hidden_below = None;
        }
        let folded = options.folded.contains(&line.id);
        if folded {
            hidden_below = Some(line.depth);
        }
        let count = list.count(&line.id);
        rows.push(BoardRow::Line {
            line: Box::new(line.clone()),
            section,
            depth: line.depth.saturating_sub(base),
            count: (count.total > 0).then_some(count),
            folded: folded && count.total > 0,
            brief: options.notes.brief(&line.id).is_some(),
        });
    }
}

/// A finished tree's row: its top line only (with its count).
fn done_row(list: &TaskList, line: &Line, options: &BoardOptions<'_>) -> BoardRow {
    let count = list.count(&line.id);
    BoardRow::Line {
        line: Box::new(line.clone()),
        section: Section::Done,
        depth: 0,
        count: (count.total > 0).then_some(count),
        folded: false,
        brief: options.notes.brief(&line.id).is_some(),
    }
}

/// The board's rows: the open trees in list order (a tree with an open line anywhere in it), then the finished trees
/// of this week (latest first) and a folded row for the older ones. Under `v`, the one tree, as it is.
pub fn board_rows(list: &TaskList, options: &BoardOptions<'_>) -> Vec<BoardRow> {
    let mut rows = Vec::new();
    match options.scope {
        BoardScope::Nothing => {
            rows.push(BoardRow::Empty {
                text: "No line for this session yet · a adds a step (it gets its line)".to_owned(),
                adds: true,
            });
            return rows;
        }
        BoardScope::Under(root) => {
            match list.get(root) {
                Some(root) => {
                    let section = if tree_done(list, root) {
                        Section::Done
                    } else {
                        Section::Open
                    };
                    push_tree(&mut rows, list, root, section, options);
                }
                None => rows.push(BoardRow::Empty {
                    text: "Its line is not in the list any more".to_owned(),
                    adds: false,
                }),
            }
            return rows;
        }
        BoardScope::All => {}
    }
    let (open, done): (Vec<&Line>, Vec<&Line>) = list.top_level().into_iter().partition(|line| !tree_done(list, line));
    rows.push(BoardRow::Heading {
        text: "OPEN".to_owned(),
        count: (!open.is_empty()).then_some(open.len()),
    });
    for root in &open {
        push_tree(&mut rows, list, root, Section::Open, options);
    }
    if open.is_empty() {
        rows.push(BoardRow::Empty {
            text: if list.lines().is_empty() {
                "No lines yet · a adds one"
            } else {
                "Nothing open · a adds a line"
            }
            .to_owned(),
            adds: true,
        });
    }
    if done.is_empty() {
        return rows;
    }
    let tasks: Vec<_> = done.iter().map(|line| line.task()).collect();
    let groups = group_tasks(&tasks, options.now_ms);
    let line_of = |id: &str| done.iter().copied().find(|line| line.id == id);
    if !groups.done.is_empty() || !groups.older_done.is_empty() {
        rows.push(BoardRow::Heading {
            text: "DONE · this week".to_owned(),
            count: (!groups.done.is_empty()).then_some(groups.done.len()),
        });
        for task in &groups.done {
            if let Some(line) = line_of(&task.id) {
                rows.push(done_row(list, line, options));
            }
        }
        if !groups.older_done.is_empty() {
            rows.push(BoardRow::Older {
                count: groups.older_done.len(),
                shown: options.show_older,
            });
            if options.show_older {
                for task in &groups.older_done {
                    if let Some(line) = line_of(&task.id) {
                        rows.push(done_row(list, line, options));
                    }
                }
            }
        }
    }
    rows
}

// ── n: starting a session on a line ──────────────────────────────────────────────────────────────────────────────

/// What `n` on a line nobody is on starts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StartOn {
    /// A new session (the usual form, in a new worktree): a top-level line, or one with no session above it.
    TopLevel,
    /// A sub-session of this session: the line is under that session's line.
    SubSessionOf(String),
}

/// Where `n` on `line_id` starts a session: a sub-session of the session on the nearest line above it, else a new
/// one.
pub fn start_on<S: SessionLike>(list: &TaskList, line_id: &str, sessions: &[S]) -> StartOn {
    list.ancestors(line_id)
        .into_iter()
        .find_map(|line| {
            let s = line.meta.s.as_deref().filter(|s| !s.is_empty())?;
            find_session(sessions, s)
                .filter(|session| !archived(session))
                .map(|session| StartOn::SubSessionOf(session.id.clone()))
        })
        .unwrap_or(StartOn::TopLevel)
}

// ── w: where a line can move ─────────────────────────────────────────────────────────────────────────────────────

/// The `w` menu's two lists (tab switches).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MoveView {
    Lines,
    Sessions,
}

/// Where a line can go.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MoveChoice {
    /// The top level, nobody on it.
    Backlog,
    /// Under this line.
    Line(String),
    /// Onto this session's work: its line, or a step of it.
    Session(String),
}

/// One row of the `w` menu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MoveOption {
    pub choice: MoveChoice,
    pub label: String,
    /// A line's depth in the list.
    pub depth: usize,
    /// What `/` searches (lower case).
    pub search: String,
}

/// Every place `moving` can go: the backlog, the open lines outside its own tree (list order), and the sessions not
/// archived (sidebar order).
pub fn move_options<S: SessionLike>(list: &TaskList, moving: &str, sessions: &[S]) -> Vec<MoveOption> {
    let own: Vec<&str> = list.subtree(moving).iter().map(|line| line.id.as_str()).collect();
    let mut options = vec![MoveOption {
        choice: MoveChoice::Backlog,
        label: "Backlog · top level, nobody on it".to_owned(),
        depth: 0,
        search: String::new(),
    }];
    options.extend(
        list.lines()
            .iter()
            .filter(|line| !line.done && !own.contains(&line.id.as_str()))
            .map(|line| MoveOption {
                choice: MoveChoice::Line(line.id.clone()),
                label: line.title.clone(),
                depth: line.depth,
                search: js_to_lower(&line.title),
            }),
    );
    for item in sort_sessions_for_sidebar(sessions) {
        let session = item.session();
        if archived(session) {
            continue;
        }
        let name = display_session_title(session, sessions);
        let label = match session
            .worktree
            .as_ref()
            .and_then(|worktree| worktree.branch.as_deref())
        {
            Some(branch) if !branch.is_empty() => format!("{name} · ⎇ {branch}"),
            _ => name,
        };
        options.push(MoveOption {
            choice: MoveChoice::Session(session.id.clone()),
            search: js_to_lower(&label),
            label,
            depth: 0,
        });
    }
    options
}

/// The menu's rows in `view` matching `query` (every word), the backlog first.
pub fn move_rows(options: &[MoveOption], view: MoveView, query: &str) -> Vec<MoveOption> {
    let lowered = js_to_lower(query);
    let terms = js_split_whitespace(&lowered);
    options
        .iter()
        .filter(|option| match (&option.choice, view) {
            (MoveChoice::Backlog, _) => true,
            (MoveChoice::Line(_), MoveView::Lines) | (MoveChoice::Session(_), MoveView::Sessions) => {
                terms.iter().all(|term| option.search.contains(term))
            }
            _ => false,
        })
        .cloned()
        .collect()
}

/// How many lines and sessions match `query` (for the tabs).
pub fn move_counts(options: &[MoveOption], query: &str) -> (usize, usize) {
    let count = |view| {
        move_rows(options, view, query)
            .iter()
            .filter(|option| option.choice != MoveChoice::Backlog)
            .count()
    };
    (count(MoveView::Lines), count(MoveView::Sessions))
}

/// What moving `line` to `choice` takes (as `deckhand task move` does); `None` when the target is gone.
pub fn move_plan<S: SessionLike>(
    list: &TaskList,
    line: &Line,
    choice: &MoveChoice,
    sessions: &[S],
) -> Option<MovePlan> {
    let target = match choice {
        MoveChoice::Backlog => MoveTarget::Backlog,
        MoveChoice::Line(id) => MoveTarget::Line(list.get(id)?),
        MoveChoice::Session(id) => MoveTarget::Session(find_session(sessions, id)?),
    };
    Some(plan_move(line, target, list))
}

// ── The Notes view ───────────────────────────────────────────────────────────────────────────────────────────────

/// A note's line as the Notes view shows it (`None` for a blank line).
pub fn note_line(line: &str) -> Option<(String, NoteLineStyle)> {
    if js_trim(line).is_empty() {
        return None;
    }
    if let Some(item) = parse_checklist_line(line) {
        let mark = if item.checked { '☑' } else { '☐' };
        let style = if item.checked {
            NoteLineStyle::Done
        } else {
            NoteLineStyle::Open
        };
        return Some((format!("{}{mark} {}", item.indent, js_trim(&item.text)), style));
    }
    if let Some(heading) = HEADING.captures(line).and_then(|captures| captures.get(1)) {
        return Some((js_trim(heading.as_str()).to_owned(), NoteLineStyle::Heading));
    }
    Some((js_trim_end(line).to_owned(), NoteLineStyle::Text))
}

/// The Notes view's options.
#[derive(Clone, Copy)]
pub struct NotesViewOptions<'a> {
    /// `f`: only notes with text.
    pub with_text: bool,
    /// `v`: the tree of the session's line (and that session's note); the scratchpad is left out.
    pub scope: BoardScope<'a>,
    /// `v`'s session.
    pub session: Option<&'a str>,
}

fn push_note(rows: &mut Vec<BoardRow>, note: NoteOf, depth: usize, text: &str) {
    for (line, raw) in text.split('\n').enumerate() {
        if let Some((text, style)) = note_line(raw) {
            rows.push(BoardRow::NoteLine {
                note: note.clone(),
                line,
                depth,
                text,
                style,
            });
        }
    }
}

/// The Notes view: the scratchpad, then line notes under their line's title (list order: every line with a note or
/// a brief, and the open lines, so one can be written), then the notes of sessions (sidebar order). `f` keeps the
/// notes with text.
pub fn notes_view_rows<S: SessionLike>(
    list: &TaskList,
    notes: &NoteMap,
    sessions: &[S],
    options: &NotesViewOptions<'_>,
) -> Vec<BoardRow> {
    let mut rows = Vec::new();
    let has_text = |text: &str| !js_trim(text).is_empty();
    if options.scope == BoardScope::All {
        let text = notes.scratchpad().map_or("", |note| note.text.as_str());
        if has_text(text) || !options.with_text {
            rows.push(BoardRow::Heading {
                text: "SCRATCHPAD".to_owned(),
                count: None,
            });
            rows.push(BoardRow::NoteHead {
                note: NoteOf::Scratchpad,
                label: "Scratchpad".to_owned(),
                depth: 0,
                done: false,
                brief: false,
                empty: !has_text(text),
            });
            push_note(&mut rows, NoteOf::Scratchpad, 0, text);
        }
    }
    let (lines, base): (Vec<&Line>, usize) = match options.scope {
        BoardScope::All => (list.lines().iter().collect(), 0),
        BoardScope::Under(root) => (list.subtree(root), list.get(root).map_or(0, |root| root.depth)),
        BoardScope::Nothing => (Vec::new(), 0),
    };
    let mut line_rows = Vec::new();
    let mut written = 0;
    for line in &lines {
        let text = notes.line_note(&line.id).map_or("", |note| note.text.as_str());
        let brief = notes.brief(&line.id).is_some();
        if !has_text(text) && (options.with_text || (line.done && !brief)) {
            continue;
        }
        written += usize::from(has_text(text));
        let depth = line.depth.saturating_sub(base);
        line_rows.push(BoardRow::NoteHead {
            note: NoteOf::Line(line.id.clone()),
            label: line.title.clone(),
            depth,
            done: line.done,
            brief,
            empty: !has_text(text),
        });
        push_note(&mut line_rows, NoteOf::Line(line.id.clone()), depth, text);
    }
    if !line_rows.is_empty() {
        rows.push(BoardRow::Heading {
            text: "LINE NOTES".to_owned(),
            count: (written > 0).then_some(written),
        });
        rows.extend(line_rows);
    }
    // Sessions: every one with a note; under v, its session's (even empty) and those on lines of its tree.
    let on_tree = |session: &SessionRecord| lines.iter().any(|line| line.meta.s.as_deref() == Some(&session.id));
    let mut session_rows = Vec::new();
    for item in sort_sessions_for_sidebar(sessions) {
        let session = item.session();
        let text = session.notes.as_deref().unwrap_or("");
        let own = options.session == Some(session.id.as_str());
        let shown = match options.scope {
            BoardScope::All => has_text(text) && !archived(session),
            _ => own && !options.with_text || has_text(text) && (own || on_tree(session)),
        };
        if !shown {
            continue;
        }
        let name = display_session_title(session, sessions);
        session_rows.push(BoardRow::NoteHead {
            note: NoteOf::Session(session.id.clone()),
            label: if name.is_empty() { "session".to_owned() } else { name },
            depth: 0,
            done: false,
            brief: false,
            empty: !has_text(text),
        });
        push_note(&mut session_rows, NoteOf::Session(session.id.clone()), 0, text);
    }
    if !session_rows.is_empty() {
        rows.push(BoardRow::Heading {
            text: "SESSION NOTES".to_owned(),
            count: None,
        });
        rows.extend(session_rows);
    }
    if rows.is_empty() {
        rows.push(BoardRow::Empty {
            text: if options.with_text {
                "No notes with text here · f shows every note"
            } else {
                "No notes here yet"
            }
            .to_owned(),
            adds: false,
        });
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jsstr::date;
    use deckhand_protocol::data::{TasksDoc, WorkNote, WorkNoteKind};
    use serde_json::{Value, json};

    fn session(id: &str, fields: Value) -> SessionRecord {
        let mut base = json!({"id": id, "title": id, "cwd": "/repo", "repoRoot": "/repo", "program": "claude",
            "command": "claude", "status": "running", "agentStatus": "idle", "createdAt": "2026-11-01T00:00:00Z",
            "updatedAt": "2026-11-01T00:00:00Z"});
        if let (Some(base), Some(fields)) = (base.as_object_mut(), fields.as_object()) {
            base.extend(fields.clone());
        }
        serde_json::from_value(base).unwrap_or_else(|error| panic!("a session: {error}"))
    }

    fn note(kind: WorkNoteKind, id: &str, text: &str) -> WorkNote {
        WorkNote {
            kind,
            id: id.into(),
            path: String::new(),
            text: text.into(),
            revision: "r".into(),
            too_large: None,
            from: None,
            at: None,
        }
    }

    fn notes(list: Vec<WorkNote>) -> NoteMap {
        let mut map = NoteMap::default();
        map.load(&TasksDoc {
            key: "k".into(),
            path: String::new(),
            text: String::new(),
            revision: "r".into(),
            too_large: None,
            notes: Some(list),
        });
        map
    }

    fn now() -> i64 {
        date::parse("2026-11-14T12:00:00Z").unwrap_or_default()
    }

    /// The walkthrough of the design: an idea an orchestrator is on, two sessions on lines under it, steps, a
    /// sub-session, a line nobody is on; a finished tree this week and an older one.
    const LIST: &str = "- [ ] Rework auth <!-- dh:t=idea s=orch -->\n\
        \x20 - [ ] Token storage <!-- dh:t=tok s=token -->\n\
        \x20   - [x] schema <!-- dh:t=sch done=2026-11-13 -->\n\
        \x20   - [ ] encryption <!-- dh:t=enc -->\n\
        \x20   - [ ] refresh tokens <!-- dh:t=ref s=refresh -->\n\
        \x20 - [ ] Login UI <!-- dh:t=ui s=login -->\n\
        \x20 - [ ] Remove legacy endpoint <!-- dh:t=leg tried=old-auth -->\n\
        - [ ] Rate-limit export <!-- dh:t=rate -->\n\
        - [x] Shipped parser <!-- dh:t=ship done=2026-11-12 auto=merge -->\n\
        - [x] Old thing <!-- dh:t=old done=2026-10-01 -->\n";

    fn sessions() -> Vec<SessionRecord> {
        vec![
            session("orch", json!({"title": "Rework auth"})),
            session("token", json!({"title": "Token storage", "parentSessionId": "orch"})),
            session(
                "refresh",
                json!({"title": "Refresh tokens", "parentSessionId": "token", "agentStatus": "active"}),
            ),
            session("login", json!({"title": "Login UI", "parentSessionId": "orch"})),
            session("loose", json!({"title": "No line yet", "parentSessionId": "token"})),
        ]
    }

    fn describe(rows: &[BoardRow]) -> Vec<String> {
        rows.iter()
            .map(|row| match row {
                BoardRow::Heading { text, count } => {
                    format!("# {text}{}", count.map_or(String::new(), |c| format!(" {c}")))
                }
                BoardRow::Line {
                    line,
                    section,
                    depth,
                    count,
                    folded,
                    brief,
                } => format!(
                    "{}{}{} {}{}{}{}",
                    "  ".repeat(*depth),
                    if *section == Section::Done { "done " } else { "" },
                    if line.done { "[x]" } else { "[ ]" },
                    line.title,
                    count.map_or(String::new(), |count| format!(" {}/{}", count.ticked, count.total)),
                    if *folded { " folded" } else { "" },
                    if *brief { " ✎" } else { "" },
                ),
                BoardRow::Older { count, shown } => format!("older {count} {shown}"),
                BoardRow::Empty { text, adds } => format!("empty {text} {adds}"),
                BoardRow::NoteHead {
                    label, depth, empty, ..
                } => format!(
                    "{}> {label}{}",
                    "  ".repeat(*depth),
                    if *empty { " (empty)" } else { "" }
                ),
                BoardRow::NoteLine { text, .. } => format!("    {text}"),
            })
            .collect()
    }

    fn options<'a>(notes: &'a NoteMap, folded: &'a [String], scope: BoardScope<'a>) -> BoardOptions<'a> {
        BoardOptions {
            show_older: false,
            now_ms: now(),
            scope,
            folded,
            notes,
        }
    }

    #[test]
    fn the_board_is_the_list_nested_with_counts_briefs_and_finished_trees_last() {
        let list = TaskList::parse(LIST);
        let notes = notes(vec![note(WorkNoteKind::Brief, "tok", "Store them encrypted")]);
        let rows = board_rows(&list, &options(&notes, &[], BoardScope::All));
        assert_eq!(
            describe(&rows),
            [
                "# OPEN 2",
                "[ ] Rework auth 0/3",
                "  [ ] Token storage 1/3 ✎",
                "    [x] schema",
                "    [ ] encryption",
                "    [ ] refresh tokens",
                "  [ ] Login UI",
                "  [ ] Remove legacy endpoint",
                "[ ] Rate-limit export",
                "# DONE · this week 1",
                "done [x] Shipped parser",
                "older 1 false",
            ]
        );
        assert_eq!(
            rows.iter().filter(|row| selectable_row(row)).count(),
            rows.len() - 2,
            "every row but the headings"
        );
        assert_eq!(row_key(&rows[2]).as_deref(), Some("line:tok"));
        // The older finished trees, shown.
        let shown = board_rows(
            &list,
            &BoardOptions {
                show_older: true,
                ..options(&notes, &[], BoardScope::All)
            },
        );
        assert_eq!(
            describe(&shown[shown.len() - 2..]),
            ["older 1 true", "done [x] Old thing"]
        );
    }

    #[test]
    fn c_hides_a_lines_lines_and_a_tree_with_an_open_line_stays_open() {
        let list = TaskList::parse(LIST);
        let notes = NoteMap::default();
        let folded = vec!["tok".to_owned()];
        let rows = board_rows(&list, &options(&notes, &folded, BoardScope::All));
        assert_eq!(
            describe(&rows[1..5]),
            [
                "[ ] Rework auth 0/3",
                "  [ ] Token storage 1/3 folded",
                "  [ ] Login UI",
                "  [ ] Remove legacy endpoint"
            ]
        );
        // Ticked by hand with a line under it still open: it stays with the open lines, in place.
        let list =
            TaskList::parse("- [x] Parent <!-- dh:t=p done=2026-11-14 -->\n  - [ ] still open <!-- dh:t=c -->\n");
        assert_eq!(
            describe(&board_rows(&list, &options(&notes, &[], BoardScope::All))),
            ["# OPEN 1", "[x] Parent 0/1", "  [ ] still open"]
        );
    }

    #[test]
    fn an_empty_list_offers_to_add_a_line() {
        let notes = NoteMap::default();
        let rows = board_rows(&TaskList::parse(""), &options(&notes, &[], BoardScope::All));
        assert_eq!(describe(&rows), ["# OPEN", "empty No lines yet · a adds one true"]);
        let all_done = TaskList::parse("- [x] Done <!-- dh:t=d done=2026-11-14 -->\n");
        assert_eq!(
            describe(&board_rows(&all_done, &options(&notes, &[], BoardScope::All))),
            [
                "# OPEN",
                "empty Nothing open · a adds a line true",
                "# DONE · this week 1",
                "done [x] Done"
            ]
        );
    }

    #[test]
    fn v_shows_the_sessions_line_else_its_nearest_ancestors() {
        let list = TaskList::parse(LIST);
        let sessions = sessions();
        assert_eq!(
            scope_root(&list, "token", &sessions).map(|line| line.id.as_str()),
            Some("tok")
        );
        // A session without a line: its parent's.
        assert_eq!(
            scope_root(&list, "loose", &sessions).map(|line| line.id.as_str()),
            Some("tok")
        );
        assert_eq!(scope_root(&list, "nobody", &sessions), None);
        let notes = NoteMap::default();
        assert_eq!(
            describe(&board_rows(&list, &options(&notes, &[], BoardScope::Under("tok")))),
            [
                "[ ] Token storage 1/3",
                "  [x] schema",
                "  [ ] encryption",
                "  [ ] refresh tokens"
            ]
        );
        assert_eq!(
            describe(&board_rows(&list, &options(&notes, &[], BoardScope::Nothing))),
            ["empty No line for this session yet · a adds a step (it gets its line) true"]
        );
    }

    #[test]
    fn who_is_on_a_line() {
        let list = TaskList::parse(LIST);
        let sessions = sessions();
        let state = |id: &str| list.get(id).map(|line| line_state(line, &sessions, "⠋"));
        let on = state("tok").unwrap_or_else(|| panic!("tok"));
        assert_eq!(
            (
                on.glyph.as_str(),
                on.who.as_deref(),
                on.text.as_str(),
                on.session_id.as_deref()
            ),
            ("●", Some("Token storage"), "idle", Some("token"))
        );
        let working = state("ref").unwrap_or_else(|| panic!("ref"));
        assert_eq!((working.glyph.as_str(), working.text.as_str()), ("⠋", "working"));
        let tried = state("leg").unwrap_or_else(|| panic!("leg"));
        assert_eq!(
            (tried.glyph.as_str(), tried.who, tried.text.as_str()),
            ("○", None, "tried in ⎇ old-auth")
        );
        let merged = state("ship").unwrap_or_else(|| panic!("ship"));
        assert_eq!(
            (merged.glyph.as_str(), merged.text.as_str()),
            ("✓", "merged 2026-11-12")
        );
        // A session that is not listed (removed elsewhere).
        let gone = TaskList::parse("- [ ] Orphan <!-- dh:t=o s=gone -->\n");
        let gone = gone.get("o").map(|line| line_state(line, &sessions, "⠋"));
        assert_eq!(gone.map(|state| state.text), Some("session not shown".to_owned()));
    }

    #[test]
    fn n_starts_a_sub_session_of_the_session_on_the_nearest_line_above() {
        let list = TaskList::parse(LIST);
        let sessions = sessions();
        assert_eq!(start_on(&list, "enc", &sessions), StartOn::SubSessionOf("token".into()));
        assert_eq!(start_on(&list, "leg", &sessions), StartOn::SubSessionOf("orch".into()));
        assert_eq!(start_on(&list, "rate", &sessions), StartOn::TopLevel);
        // Nobody listed above it: a new session.
        let orphan = TaskList::parse("- [ ] Gone's <!-- dh:t=g s=gone -->\n  - [ ] step <!-- dh:t=s -->\n");
        assert_eq!(start_on(&orphan, "s", &sessions), StartOn::TopLevel);
    }

    #[test]
    fn w_lists_the_backlog_open_lines_outside_its_tree_and_sessions() {
        let list = TaskList::parse(LIST);
        let sessions = sessions();
        let options = move_options(&list, "tok", &sessions);
        let labels = |view, query| -> Vec<String> {
            move_rows(&options, view, query)
                .into_iter()
                .map(|option| format!("{}{}", "  ".repeat(option.depth), option.label))
                .collect()
        };
        assert_eq!(
            labels(MoveView::Lines, ""),
            [
                "Backlog · top level, nobody on it",
                "Rework auth",
                "  Login UI",
                "  Remove legacy endpoint",
                "Rate-limit export",
            ]
        );
        assert_eq!(
            labels(MoveView::Sessions, "login"),
            ["Backlog · top level, nobody on it", "Login UI"]
        );
        assert_eq!(move_counts(&options, ""), (4, 5));
        let line = list.get("rate").cloned().unwrap_or_else(|| panic!("rate"));
        // Onto a session with a line: under it; onto one without: its line; under a line; to the backlog.
        assert_eq!(
            move_plan(&list, &line, &MoveChoice::Session("login".into()), &sessions),
            Some(MovePlan::MoveTo {
                parent: Some("ui".into()),
                unhook: None
            })
        );
        assert_eq!(
            move_plan(&list, &line, &MoveChoice::Session("loose".into()), &sessions),
            Some(MovePlan::SessionOn { s: "loose".into() })
        );
        assert_eq!(
            move_plan(&list, &line, &MoveChoice::Backlog, &sessions),
            Some(MovePlan::Already)
        );
        assert_eq!(
            move_plan(&list, &line, &MoveChoice::Line("gone".into()), &sessions),
            None
        );
    }

    #[test]
    fn the_notes_view_lists_the_scratchpad_line_notes_and_session_notes() {
        let list = TaskList::parse(LIST);
        let mut sessions = sessions();
        sessions[1].notes = Some("Review crypto.rs\n".into());
        let map = notes(vec![
            note(WorkNoteKind::Scratchpad, "k", "# Ideas\n- [ ] cache\n\nloose thought\n"),
            note(WorkNoteKind::Line, "tok", "- [x] chose AES\nfindings"),
            note(WorkNoteKind::Brief, "ship", "what it was"),
        ]);
        let view = |with_text, scope, session| {
            describe(&notes_view_rows(
                &list,
                &map,
                &sessions,
                &NotesViewOptions {
                    with_text,
                    scope,
                    session,
                },
            ))
        };
        assert_eq!(
            view(false, BoardScope::All, None),
            [
                "# SCRATCHPAD",
                "> Scratchpad",
                "    Ideas",
                "    ☐ cache",
                "    loose thought",
                "# LINE NOTES 1",
                "> Rework auth (empty)",
                "  > Token storage",
                "    ☑ chose AES",
                "    findings",
                "    > encryption (empty)",
                "    > refresh tokens (empty)",
                "  > Login UI (empty)",
                "  > Remove legacy endpoint (empty)",
                "> Rate-limit export (empty)",
                "> Shipped parser (empty)",
                "# SESSION NOTES",
                "> Token storage",
                "    Review crypto.rs",
            ]
        );
        // f: only notes with text.
        assert_eq!(
            view(true, BoardScope::All, None),
            [
                "# SCRATCHPAD",
                "> Scratchpad",
                "    Ideas",
                "    ☐ cache",
                "    loose thought",
                "# LINE NOTES 1",
                "  > Token storage",
                "    ☑ chose AES",
                "    findings",
                "# SESSION NOTES",
                "> Token storage",
                "    Review crypto.rs",
            ]
        );
        // v: the session's tree and notes (its own even when empty), not the scratchpad.
        assert_eq!(
            view(false, BoardScope::Under("tok"), Some("loose")),
            [
                "# LINE NOTES 1",
                "> Token storage",
                "    ☑ chose AES",
                "    findings",
                "  > encryption (empty)",
                "  > refresh tokens (empty)",
                "# SESSION NOTES",
                "> Token storage",
                "    Review crypto.rs",
                "> No line yet (empty)",
            ]
        );
        assert_eq!(
            view(true, BoardScope::Nothing, Some("nobody")),
            ["empty No notes with text here · f shows every note false"]
        );
    }
}
