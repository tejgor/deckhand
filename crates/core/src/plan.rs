//! Orchestrator plans (`deckhand plan check|apply`, not in the TS): the JSON an orchestrator writes, validated against
//! the repository's sessions and tasks, ordered for creation (parents first) and summarised for the confirmation.
//!
//! ```json
//! {"sessions": [{"key": "auth", "title": "Auth rework", "prompt": "…", "agent": "claude", "parent": null,
//!                "worktree": "new", "base": null, "flat": false, "task": null},
//!               {"key": "tokens", "title": "Token storage", "prompt": "…", "parent": "auth"}],
//!  "tasks": [{"title": "Remove legacy endpoint", "details": "After Auth rework merges: …"}]}
//! ```
//!
//! Validation reports every problem at once, each naming its entry, so a plan is fixed in one go. Nothing here does
//! I/O: what it needs from Git and the daemon comes in [`PlanContext`].

use std::collections::{HashMap, HashSet};

use deckhand_protocol::data::{
    Plan, PlanSession, PlanSummary, PlanSummaryEntry, PlanTask, PlanWorktree, ProgramKey, SessionRecord, SessionStatus,
    Task,
};
use serde_json::{Map, Value};

use crate::jsstr::{js_collapse_whitespace, js_len, js_trim};

/// The longest session title the daemon takes (UTF-16 units).
pub const MAX_TITLE: usize = 64;

/// The keys a plan session may have.
pub const SESSION_KEYS: &[&str] = &[
    "key", "title", "prompt", "agent", "parent", "worktree", "base", "flat", "task",
];

/// The keys a plan task may have.
pub const TASK_KEYS: &[&str] = &["title", "details"];

/// What validation needs besides the plan.
#[derive(Clone, Copy, Debug)]
pub struct PlanContext<'a> {
    /// The repository's sessions (with their paths projected, as the daemon holds them).
    pub sessions: &'a [SessionRecord],
    /// The repository's lines, at every depth (a session may start on any open line nobody is on).
    pub tasks: &'a [Task],
    /// The repository's `defaultAgent`.
    pub default_agent: Option<ProgramKey>,
    /// For existing sessions: the branch a new worktree under them stacks on (their live linked worktree's branch).
    pub stack_branches: &'a HashMap<String, String>,
    /// `base` values that name no commit in the repository.
    pub missing_bases: &'a [String],
}

/// A validated plan session's parent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlanParent {
    /// Another session of the plan, by its index in [`ValidPlan::plan`] (always earlier).
    Entry(usize),
    /// An existing session, by its full ID.
    Session(String),
}

/// A plan that passed validation: sessions in creation order (each after its parent), their parents resolved, and
/// the summary (its sessions in the same order).
#[derive(Clone, Debug, PartialEq)]
pub struct ValidPlan {
    pub plan: Plan,
    pub parents: Vec<Option<PlanParent>>,
    pub summary: PlanSummary,
}

/// A plan session as read, before its parent is resolved.
#[derive(Clone, Debug, Default)]
struct Entry {
    label: String,
    key: Option<String>,
    title: Option<String>,
    prompt: Option<String>,
    agent: Option<ProgramKey>,
    parent: Option<String>,
    worktree: Option<PlanWorktree>,
    base: Option<String>,
    flat: Option<bool>,
    task: Option<String>,
}

/// A parent as named, resolved.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Named {
    Entry(usize),
    Session(String),
}

/// `sessions[1] (tokens)`.
fn label(index: usize, key: Option<&str>) -> String {
    match key {
        Some(key) => format!("sessions[{index}] ({key})"),
        None => format!("sessions[{index}]"),
    }
}

/// A string field: absent and `null` are none; anything but text is a problem.
fn text_field(object: &Map<String, Value>, name: &str, at: &str, problems: &mut Vec<String>) -> Option<String> {
    match object.get(name) {
        None | Some(Value::Null) => None,
        Some(Value::String(text)) => Some(text.clone()),
        Some(_) => {
            problems.push(format!("{at}: {name} must be text"));
            None
        }
    }
}

/// Keys of `object` not in `allowed`.
fn unknown_keys(object: &Map<String, Value>, allowed: &[&str], at: &str, problems: &mut Vec<String>) {
    for key in object.keys().filter(|key| !allowed.contains(&key.as_str())) {
        problems.push(format!("{at}: unknown key \"{key}\" (allowed: {})", allowed.join(", ")));
    }
}

/// The item whose ID is `id`, or the only one it is a prefix of; `Err` says why not.
fn find_by_id<'a, T>(id: &str, items: &'a [T], id_of: impl Fn(&T) -> &str) -> Result<&'a T, &'static str> {
    if let Some(exact) = items.iter().find(|item| id_of(item) == id) {
        return Ok(exact);
    }
    let mut matches = items
        .iter()
        .filter(|item| !id.is_empty() && id_of(item).starts_with(id));
    match (matches.next(), matches.next()) {
        (Some(only), None) => Ok(only),
        (Some(_), Some(_)) => Err("ambiguous"),
        _ => Err("missing"),
    }
}

/// A session's path (`parent / child`).
fn path_of(session: &SessionRecord) -> &str {
    session.path_title.as_deref().unwrap_or(&session.title)
}

/// Reads one plan session, adding what is wrong with it to `problems`.
fn read_session(index: usize, value: &Value, problems: &mut Vec<String>) -> Entry {
    let Some(object) = value.as_object() else {
        problems.push(format!("{}: must be an object", label(index, None)));
        return Entry {
            label: label(index, None),
            ..Entry::default()
        };
    };
    let key = match object.get("key") {
        Some(Value::String(key)) if !js_trim(key).is_empty() => Some(js_trim(key).to_owned()),
        _ => None,
    };
    let at = label(index, key.as_deref());
    if key.is_none() {
        problems.push(format!(
            "{at}: key is required (a short name other sessions use as their parent)"
        ));
    }
    unknown_keys(object, SESSION_KEYS, &at, problems);
    let title = text_field(object, "title", &at, problems).map(|title| js_collapse_whitespace(js_trim(&title)));
    match title.as_deref() {
        None | Some("") => problems.push(format!("{at}: title is required")),
        Some(title) if js_len(title) > MAX_TITLE => {
            problems.push(format!("{at}: title is longer than {MAX_TITLE} characters"));
        }
        Some(_) => {}
    }
    let prompt = text_field(object, "prompt", &at, problems);
    if prompt.as_deref().is_none_or(|prompt| js_trim(prompt).is_empty()) {
        problems.push(format!(
            "{at}: prompt is required (the session's complete brief, sent as its first message)"
        ));
    }
    let agent = text_field(object, "agent", &at, problems).and_then(|agent| {
        let found = ProgramKey::ALL
            .iter()
            .copied()
            .find(|program| program.as_str() == agent);
        if found.is_none() {
            problems.push(format!("{at}: unknown agent \"{agent}\" (claude, codex or pi)"));
        }
        found
    });
    let worktree = text_field(object, "worktree", &at, problems).and_then(|mode| {
        let found = PlanWorktree::ALL.iter().copied().find(|known| known.as_str() == mode);
        if found.is_none() {
            problems.push(format!("{at}: unknown worktree \"{mode}\" (new, same or none)"));
        }
        found
    });
    let flat = match object.get("flat") {
        None | Some(Value::Null) => None,
        Some(Value::Bool(flat)) => Some(*flat),
        Some(_) => {
            problems.push(format!("{at}: flat must be true or false"));
            None
        }
    };
    Entry {
        label: at.clone(),
        key,
        title,
        prompt,
        agent,
        parent: text_field(object, "parent", &at, problems).filter(|parent| !parent.is_empty()),
        worktree,
        base: text_field(object, "base", &at, problems).filter(|base| !base.is_empty()),
        flat,
        task: text_field(object, "task", &at, problems).filter(|task| !task.is_empty()),
    }
}

/// Reads one plan task.
fn read_task(index: usize, value: &Value, problems: &mut Vec<String>) -> Option<PlanTask> {
    let at = format!("tasks[{index}]");
    let Some(object) = value.as_object() else {
        problems.push(format!("{at}: must be an object"));
        return None;
    };
    unknown_keys(object, TASK_KEYS, &at, problems);
    let title = text_field(object, "title", &at, problems)
        .map(|title| js_collapse_whitespace(js_trim(&title)))
        .filter(|title| !title.is_empty());
    if title.is_none() {
        problems.push(format!("{at}: title is required"));
    }
    let details = text_field(object, "details", &at, problems).filter(|details| !js_trim(details).is_empty());
    title.map(|title| PlanTask { title, details })
}

/// What a new worktree under a session stacks on.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Stack {
    /// Nothing: it is flat.
    Flat,
    /// The new branch of the plan session with this title (created by the plan).
    NewBranch(String),
    /// An existing branch.
    Branch(String),
}

/// Validates `raw` (the plan JSON) against the repository (see the module docs for the format). Every problem is
/// reported, each naming its entry.
pub fn validate_plan(raw: &Value, cx: &PlanContext<'_>) -> Result<ValidPlan, Vec<String>> {
    let mut problems = Vec::new();
    let Some(top) = raw.as_object() else {
        return Err(vec![
            "the plan must be a JSON object with \"sessions\" and \"tasks\"".to_owned(),
        ]);
    };
    unknown_keys(top, &["sessions", "tasks"], "the plan", &mut problems);
    let list = |name: &str, problems: &mut Vec<String>| match top.get(name) {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(items)) => items.clone(),
        Some(_) => {
            problems.push(format!("the plan: {name} must be a list"));
            Vec::new()
        }
    };
    let raw_sessions = list("sessions", &mut problems);
    let raw_tasks = list("tasks", &mut problems);
    if raw_sessions.is_empty() && raw_tasks.is_empty() && problems.is_empty() {
        problems.push("the plan has no sessions and no tasks".to_owned());
    }
    let entries: Vec<Entry> = raw_sessions
        .iter()
        .enumerate()
        .map(|(index, value)| read_session(index, value, &mut problems))
        .collect();
    let tasks: Vec<PlanTask> = raw_tasks
        .iter()
        .enumerate()
        .filter_map(|(index, value)| read_task(index, value, &mut problems))
        .collect();

    // Keys are unique.
    let mut by_key: HashMap<&str, usize> = HashMap::new();
    for (index, entry) in entries.iter().enumerate() {
        let Some(key) = entry.key.as_deref() else { continue };
        if let Some(first) = by_key.get(key) {
            problems.push(format!(
                "{}: key \"{key}\" is already used by sessions[{first}]",
                entry.label
            ));
        } else {
            by_key.insert(key, index);
        }
    }

    // Parents: a key of this plan, else an existing session of the repository (any of them: the user confirms).
    let named: Vec<Option<Named>> = entries
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            let parent = entry.parent.as_deref()?;
            if let Some(&found) = by_key.get(parent) {
                if found == index {
                    problems.push(format!("{}: its parent is itself", entry.label));
                    return None;
                }
                return Some(Named::Entry(found));
            }
            match find_by_id(parent, cx.sessions, |session| &session.id) {
                Ok(session) => Some(Named::Session(session.id.clone())),
                Err("ambiguous") => {
                    problems.push(format!(
                        "{}: parent \"{parent}\" is the start of more than one session ID; give more of it",
                        entry.label
                    ));
                    None
                }
                Err(_) => {
                    problems.push(format!(
                        "{}: parent \"{parent}\" is neither a key in this plan nor a session of this repository (see \
                         deckhand ls)",
                        entry.label
                    ));
                    None
                }
            }
        })
        .collect();

    // No loops through parents: each loop is reported once, at its first entry.
    let mut in_loop: HashSet<usize> = HashSet::new();
    for start in 0..entries.len() {
        if in_loop.contains(&start) {
            continue;
        }
        let mut chain = vec![start];
        let mut at = start;
        while let Some(Named::Entry(next)) = named.get(at).cloned().flatten() {
            if let Some(position) = chain.iter().position(|seen| *seen == next) {
                let cycle: Vec<usize> = chain[position..].to_vec();
                if cycle.iter().all(|index| !in_loop.contains(index)) {
                    let keys: Vec<&str> = cycle
                        .iter()
                        .chain(std::iter::once(&next))
                        .map(|index| entries[*index].key.as_deref().unwrap_or("?"))
                        .collect();
                    problems.push(format!(
                        "{}: parents go round in a loop ({})",
                        entries[cycle[0]].label,
                        keys.join(" → ")
                    ));
                    in_loop.extend(cycle);
                }
                break;
            }
            chain.push(next);
            at = next;
        }
    }

    // Worktree modes, bases, flat and tasks.
    let mut used_tasks: HashMap<String, usize> = HashMap::new();
    let mut resolved_tasks: Vec<Option<(String, String)>> = vec![None; entries.len()];
    for (index, entry) in entries.iter().enumerate() {
        let worktree = entry.worktree.unwrap_or(PlanWorktree::New);
        let has_parent = entry.parent.is_some();
        if worktree == PlanWorktree::Same && !has_parent {
            problems.push(format!(
                "{}: worktree \"same\" needs a parent (it runs in its parent's worktree)",
                entry.label
            ));
        }
        if worktree != PlanWorktree::New {
            if entry.base.is_some() {
                problems.push(format!("{}: base applies only to worktree \"new\"", entry.label));
            }
            if entry.flat.is_some() {
                problems.push(format!("{}: flat applies only to worktree \"new\"", entry.label));
            }
        }
        if let Some(base) = entry.base.as_deref()
            && cx.missing_bases.iter().any(|missing| missing == base)
        {
            problems.push(format!(
                "{}: base \"{base}\" is not a branch or commit of this repository",
                entry.label
            ));
        }
        let Some(task_id) = entry.task.as_deref() else { continue };
        match find_by_id(task_id, cx.tasks, |task| &task.id) {
            Err("ambiguous") => problems.push(format!(
                "{}: task \"{task_id}\" is the start of more than one task ID; give more of it",
                entry.label
            )),
            Err(_) => problems.push(format!(
                "{}: no task \"{task_id}\" (see deckhand task list)",
                entry.label
            )),
            Ok(task) if task.done => {
                problems.push(format!("{}: task \"{}\" is done", entry.label, task.title));
            }
            Ok(task)
                if task.meta.wt.as_deref().is_some_and(|wt| !wt.is_empty())
                    || task.meta.s.as_deref().is_some_and(|s| !s.is_empty()) =>
            {
                problems.push(format!(
                    "{}: task \"{}\" is already being worked on",
                    entry.label, task.title
                ));
            }
            Ok(task) => {
                if let Some(first) = used_tasks.get(&task.id) {
                    problems.push(format!(
                        "{}: task \"{}\" is already started by sessions[{first}]",
                        entry.label, task.title
                    ));
                } else {
                    used_tasks.insert(task.id.clone(), index);
                    resolved_tasks[index] = Some((task.id.clone(), task.title.clone()));
                }
            }
        }
    }

    if !problems.is_empty() {
        return Err(problems);
    }

    // Tree order: roots in plan order (top-level, or under an existing session), each followed by its children.
    let mut order = Vec::with_capacity(entries.len());
    let mut depths = vec![0u32; entries.len()];
    fn visit(at: usize, depth: u32, named: &[Option<Named>], order: &mut Vec<usize>, depths: &mut [u32]) {
        order.push(at);
        depths[at] = depth;
        for (child, parent) in named.iter().enumerate() {
            if *parent == Some(Named::Entry(at)) {
                visit(child, depth + 1, named, order, depths);
            }
        }
    }
    for (index, parent) in named.iter().enumerate() {
        if !matches!(parent, Some(Named::Entry(_))) {
            visit(index, 0, &named, &mut order, &mut depths);
        }
    }
    let position: HashMap<usize, usize> = order.iter().enumerate().map(|(at, index)| (*index, at)).collect();

    // Paths, for clashes with active sessions and between siblings.
    let mut paths: Vec<String> = vec![String::new(); entries.len()];
    for &index in &order {
        let title = entries[index].title.clone().unwrap_or_default();
        paths[index] = match &named[index] {
            None => title,
            Some(Named::Entry(parent)) => format!("{} / {title}", paths[*parent]),
            Some(Named::Session(id)) => {
                let parent = cx.sessions.iter().find(|session| session.id == *id);
                format!("{} / {title}", parent.map(path_of).unwrap_or_default())
            }
        };
    }
    let mut seen: HashMap<&str, usize> = HashMap::new();
    for &index in &order {
        let path = paths[index].as_str();
        if let Some(first) = seen.get(path) {
            problems.push(format!(
                "{}: sessions[{first}] has the same name under the same parent (\"{path}\")",
                entries[index].label
            ));
            continue;
        }
        seen.insert(path, index);
        if cx
            .sessions
            .iter()
            .any(|session| session.status != SessionStatus::Exited && path_of(session) == path)
        {
            problems.push(format!(
                "{}: an active session named \"{path}\" already exists in this repository",
                entries[index].label
            ));
        }
    }
    if !problems.is_empty() {
        return Err(problems);
    }

    let agent_default = cx.default_agent.unwrap_or(ProgramKey::Claude);
    let mut sessions = Vec::with_capacity(order.len());
    let mut parents = Vec::with_capacity(order.len());
    let mut summary_entries = Vec::with_capacity(order.len());
    // How a new worktree under each entry stacks: on its own new branch (its title), on an existing branch, or not.
    let mut child_stack: HashMap<usize, Stack> = HashMap::new();
    for &index in &order {
        let entry = &entries[index];
        let worktree = entry.worktree.unwrap_or(PlanWorktree::New);
        let parent = named[index].clone();
        let session_parent = match &parent {
            Some(Named::Session(id)) => cx.sessions.iter().find(|session| session.id == *id),
            _ => None,
        };
        let inherited = |parent: &Option<Named>| match parent {
            Some(Named::Entry(parent)) => child_stack.get(parent).cloned().unwrap_or(Stack::Flat),
            Some(Named::Session(id)) => match cx.stack_branches.get(id) {
                Some(branch) => Stack::Branch(branch.clone()),
                None => Stack::Flat,
            },
            None => Stack::Flat,
        };
        let under = inherited(&parent);
        let own = match worktree {
            PlanWorktree::New => Stack::NewBranch(entry.title.clone().unwrap_or_default()),
            PlanWorktree::Same => under.clone(),
            PlanWorktree::None => Stack::Flat,
        };
        let (stacked, stacked_on, stacked_on_title) = if worktree != PlanWorktree::New {
            (None, None, None)
        } else if entry.flat == Some(true) || parent.is_none() {
            (Some(false), None, None)
        } else {
            match under {
                // A parent's new branch does not exist yet: a base names another start, so flat.
                Stack::NewBranch(title) if entry.base.is_none() => (Some(true), None, Some(title)),
                Stack::Branch(branch) if entry.base.as_deref().is_none_or(|base| base == branch) => {
                    (Some(true), Some(branch), None)
                }
                _ => (Some(false), None, None),
            }
        };
        child_stack.insert(index, own);
        let parent_title = match &parent {
            Some(Named::Entry(parent)) => entries[*parent].title.clone(),
            Some(Named::Session(_)) => session_parent.map(|session| path_of(session).to_owned()),
            None => None,
        };
        let task = resolved_tasks[index].clone();
        let key = entry.key.clone().unwrap_or_default();
        let title = entry.title.clone().unwrap_or_default();
        let agent = entry.agent.unwrap_or(agent_default);
        summary_entries.push(PlanSummaryEntry {
            key: key.clone(),
            title: title.clone(),
            depth: depths[index],
            agent,
            worktree,
            parent_key: match &parent {
                Some(Named::Entry(parent)) => entries[*parent].key.clone(),
                _ => None,
            },
            parent_session_id: match &parent {
                Some(Named::Session(id)) => Some(id.clone()),
                _ => None,
            },
            parent_title,
            base: entry.base.clone(),
            stacked,
            stacked_on,
            stacked_on_title,
            task_id: task.as_ref().map(|(id, _)| id.clone()),
            task_title: task.as_ref().map(|(_, title)| title.clone()),
        });
        parents.push(match &parent {
            None => None,
            Some(Named::Entry(parent)) => Some(PlanParent::Entry(position[parent])),
            Some(Named::Session(id)) => Some(PlanParent::Session(id.clone())),
        });
        sessions.push(PlanSession {
            key,
            title,
            prompt: entry.prompt.clone().unwrap_or_default(),
            agent,
            parent: match &parent {
                None => None,
                Some(Named::Entry(parent)) => entries[*parent].key.clone(),
                Some(Named::Session(id)) => Some(id.clone()),
            },
            worktree,
            base: entry.base.clone(),
            flat: entry.flat,
            task: task.map(|(id, _)| id),
        });
    }
    let counts = plan_counts(sessions.len(), tasks.len());
    Ok(ValidPlan {
        summary: PlanSummary {
            sessions: summary_entries,
            tasks: tasks.clone(),
            counts,
        },
        plan: Plan { sessions, tasks },
        parents,
    })
}

/// `3 sessions, 1 task` (a zero count is left out, unless both are).
pub fn plan_counts(sessions: usize, tasks: usize) -> String {
    let count = |n: usize, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
    match (sessions, tasks) {
        (0, 0) => "nothing".to_owned(),
        (sessions, 0) => count(sessions, "session", "sessions"),
        (0, tasks) => count(tasks, "task", "tasks"),
        (sessions, tasks) => format!(
            "{}, {}",
            count(sessions, "session", "sessions"),
            count(tasks, "task", "tasks")
        ),
    }
}

/// The message for a plan that did not validate: how many problems, then one per line.
pub fn plan_problems_message(problems: &[String]) -> String {
    let heading = if problems.len() == 1 {
        "The plan has a problem (nothing was created):".to_owned()
    } else {
        format!("The plan has {} problems (nothing was created):", problems.len())
    };
    let mut lines = vec![heading];
    lines.extend(problems.iter().map(|problem| format!("- {problem}")));
    lines.join("\n")
}

/// Where a summary entry works, in words: `new worktree from main`, `stacked on auth`, `same worktree`, ...
pub fn entry_placement(entry: &PlanSummaryEntry) -> String {
    match entry.worktree {
        PlanWorktree::Same => "in its parent's worktree".to_owned(),
        PlanWorktree::None => "no worktree".to_owned(),
        PlanWorktree::New => match entry.stacked {
            Some(true) => match (entry.stacked_on.as_deref(), entry.stacked_on_title.as_deref()) {
                (Some(branch), _) => format!("stacked on {branch}"),
                (None, Some(owner)) => format!("stacked on {owner}'s branch"),
                (None, None) => "stacked".to_owned(),
            },
            _ => {
                let flat = if entry.parent_key.is_some() || entry.parent_session_id.is_some() {
                    "flat new worktree"
                } else {
                    "new worktree"
                };
                match entry.base.as_deref() {
                    Some(base) => format!("{flat} from {base}"),
                    None => flat.to_owned(),
                }
            }
        },
    }
}

/// One session of a plan's tree as the confirmation lays it out: `indent` columns, then the marker (`◇` for a root,
/// `↳` for a child), the title, and in `detail` where it works, its agent and its task.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanEntryLine {
    pub indent: usize,
    pub marker: &'static str,
    pub title: String,
    /// `new worktree · claude`, `under Fix login · stacked on fix-login · codex · task: …`.
    pub detail: String,
}

/// [`PlanEntryLine`] of `entry`: two columns a level.
pub fn entry_line(entry: &PlanSummaryEntry) -> PlanEntryLine {
    let mut words = Vec::new();
    if entry.depth == 0
        && let Some(parent) = entry.parent_title.as_deref()
    {
        words.push(format!("under {parent}"));
    }
    words.push(entry_placement(entry));
    words.push(entry.agent.as_str().to_owned());
    if let Some(task) = entry.task_title.as_deref() {
        words.push(format!("task: {task}"));
    }
    PlanEntryLine {
        indent: 2 * usize::try_from(entry.depth).unwrap_or(0),
        marker: if entry.depth == 0 { "◇" } else { "↳" },
        title: entry.title.clone(),
        detail: words.join(" · "),
    }
}

/// The summary as text lines, for the terminal and the confirmation: the tree ([`entry_line`]: `◇ Title  (where ·
/// agent)`); the tasks; the counts.
pub fn summary_lines(summary: &PlanSummary) -> Vec<String> {
    let mut lines = Vec::new();
    for entry in &summary.sessions {
        let line = entry_line(entry);
        lines.push(format!(
            "{}{} {}  ({})",
            " ".repeat(line.indent),
            line.marker,
            line.title,
            line.detail
        ));
    }
    for task in &summary.tasks {
        lines.push(format!("☐ Backlog: {}", task.title));
    }
    lines.push(summary.counts.clone());
    lines
}

/// When a pending plan expiring at `expires_at` (ISO) is dropped unanswered, seen at `now_ms`: `expires in 14 min`,
/// `expires in under a minute`; `None` when `expires_at` does not parse.
pub fn expiry_text(expires_at: &str, now_ms: i64) -> Option<String> {
    let left = crate::jsstr::date::parse(expires_at)? - now_ms;
    if left < 60_000 {
        return Some("expires in under a minute".to_owned());
    }
    // Whole minutes, rounded down: never more time than there is.
    Some(format!("expires in {} min", left / 60_000))
}

#[cfg(test)]
mod tests {
    use super::*;
    use deckhand_protocol::data::TaskMeta;
    use serde_json::json;

    fn session(id: &str, title: &str, status: &str) -> SessionRecord {
        serde_json::from_value(json!({
            "id": id, "title": title, "program": "claude", "command": "claude", "cwd": "/r", "repoRoot": "/r",
            "status": status, "createdAt": "", "updatedAt": "",
        }))
        .expect("a session")
    }

    fn task(id: &str, title: &str) -> Task {
        Task {
            id: id.to_owned(),
            title: title.to_owned(),
            done: false,
            body: String::new(),
            meta: TaskMeta {
                t: Some(id.to_owned()),
                ..TaskMeta::default()
            },
        }
    }

    struct Fixture {
        sessions: Vec<SessionRecord>,
        tasks: Vec<Task>,
        stack: HashMap<String, String>,
        missing: Vec<String>,
        default_agent: Option<ProgramKey>,
    }

    impl Fixture {
        fn new() -> Self {
            let mut lead = session("1f2e3d4c-lead", "Fix login", "running");
            lead.path_title = Some("Fix login".into());
            Fixture {
                sessions: vec![lead, session("9a8b7c6d-old", "Old", "exited")],
                tasks: vec![task("ab12cd34", "Remove the legacy endpoint")],
                stack: HashMap::from([("1f2e3d4c-lead".to_owned(), "fix-login".to_owned())]),
                missing: vec!["nope".to_owned()],
                default_agent: None,
            }
        }

        fn cx(&self) -> PlanContext<'_> {
            PlanContext {
                sessions: &self.sessions,
                tasks: &self.tasks,
                default_agent: self.default_agent,
                stack_branches: &self.stack,
                missing_bases: &self.missing,
            }
        }

        fn valid(&self, plan: Value) -> ValidPlan {
            validate_plan(&plan, &self.cx()).unwrap_or_else(|problems| panic!("{problems:#?}"))
        }

        fn problems(&self, plan: Value) -> Vec<String> {
            match validate_plan(&plan, &self.cx()) {
                Ok(valid) => panic!("valid: {valid:#?}"),
                Err(problems) => problems,
            }
        }
    }

    fn entry(key: &str, extra: Value) -> Value {
        let mut value = json!({"key": key, "title": key, "prompt": format!("Brief for {key}")});
        if let (Some(value), Some(extra)) = (value.as_object_mut(), extra.as_object()) {
            value.extend(extra.clone());
        }
        value
    }

    #[test]
    fn a_plan_is_ordered_parents_first_with_defaults_filled_in() {
        let fixture = Fixture::new();
        let valid = fixture.valid(json!({
            "sessions": [
                entry("tokens", json!({"parent": "auth", "agent": "codex"})),
                entry("auth", json!({"title": "  Auth   rework "})),
                entry("ui", json!({"parent": "auth", "worktree": "same"})),
                entry("tests", json!({"parent": "1f2e", "task": "ab12"})),
            ],
            "tasks": [{"title": "Remove legacy endpoint", "details": "After Auth rework merges"}],
        }));
        let keys: Vec<&str> = valid.plan.sessions.iter().map(|session| session.key.as_str()).collect();
        assert_eq!(keys, ["auth", "tokens", "ui", "tests"]);
        assert_eq!(
            valid.parents,
            [
                None,
                Some(PlanParent::Entry(0)),
                Some(PlanParent::Entry(0)),
                Some(PlanParent::Session("1f2e3d4c-lead".into()))
            ]
        );
        let auth = &valid.plan.sessions[0];
        assert_eq!(auth.title, "Auth rework");
        assert_eq!((auth.agent, auth.worktree), (ProgramKey::Claude, PlanWorktree::New));
        assert_eq!(valid.plan.sessions[1].agent, ProgramKey::Codex);
        assert_eq!(valid.plan.sessions[1].parent.as_deref(), Some("auth"));
        // An existing parent and a task are named by their full IDs.
        assert_eq!(valid.plan.sessions[3].parent.as_deref(), Some("1f2e3d4c-lead"));
        assert_eq!(valid.plan.sessions[3].task.as_deref(), Some("ab12cd34"));
        assert_eq!(valid.plan.tasks.len(), 1);
    }

    #[test]
    fn the_agent_defaults_to_the_repositorys_default_agent() {
        let mut fixture = Fixture::new();
        fixture.default_agent = Some(ProgramKey::Pi);
        let valid = fixture.valid(json!({"sessions": [entry("a", json!({}))]}));
        assert_eq!(valid.plan.sessions[0].agent, ProgramKey::Pi);
    }

    #[test]
    fn every_problem_is_reported_at_once_naming_its_entry() {
        let fixture = Fixture::new();
        let problems = fixture.problems(json!({
            "sessions": [
                {"title": "No key", "prompt": "x"},
                {"key": "a", "title": "", "prompt": " "},
                {"key": "b", "title": "x".repeat(65), "prompt": "x", "agent": "gpt", "worktree": "copy", "colour": 1},
                entry("b", json!({})),
                entry("c", json!({"parent": "zz"})),
                entry("d", json!({"worktree": "same"})),
                entry("e", json!({"worktree": "none", "base": "main", "flat": true})),
                entry("f", json!({"base": "nope", "flat": "yes"})),
                entry("g", json!({"task": "zz"})),
                entry("h", json!({"parent": "1f2e", "worktree": "same", "task": "ab12"})),
                entry("i", json!({"parent": "i"})),
            ],
            "tasks": [{"details": "x"}, {"title": "ok", "when": "later"}],
            "notes": "",
        }));
        let expected = [
            "the plan: unknown key \"notes\" (allowed: sessions, tasks)",
            "sessions[0]: key is required (a short name other sessions use as their parent)",
            "sessions[1] (a): title is required",
            "sessions[1] (a): prompt is required (the session's complete brief, sent as its first message)",
            "sessions[2] (b): unknown key \"colour\" (allowed: key, title, prompt, agent, parent, worktree, base, flat, task)",
            "sessions[2] (b): title is longer than 64 characters",
            "sessions[2] (b): unknown agent \"gpt\" (claude, codex or pi)",
            "sessions[2] (b): unknown worktree \"copy\" (new, same or none)",
            "sessions[7] (f): flat must be true or false",
            "tasks[0]: title is required",
            "tasks[1]: unknown key \"when\" (allowed: title, details)",
            "sessions[3] (b): key \"b\" is already used by sessions[2]",
            "sessions[4] (c): parent \"zz\" is neither a key in this plan nor a session of this repository (see deckhand ls)",
            "sessions[10] (i): its parent is itself",
            "sessions[5] (d): worktree \"same\" needs a parent (it runs in its parent's worktree)",
            "sessions[6] (e): base applies only to worktree \"new\"",
            "sessions[6] (e): flat applies only to worktree \"new\"",
            "sessions[7] (f): base \"nope\" is not a branch or commit of this repository",
            "sessions[8] (g): no task \"zz\" (see deckhand task list)",
        ];
        assert_eq!(problems, expected);
    }

    #[test]
    fn a_plan_must_be_an_object_with_something_in_it() {
        let fixture = Fixture::new();
        assert_eq!(
            fixture.problems(json!([1])),
            ["the plan must be a JSON object with \"sessions\" and \"tasks\""]
        );
        assert_eq!(fixture.problems(json!({})), ["the plan has no sessions and no tasks"]);
        assert_eq!(
            fixture.problems(json!({"sessions": {}, "tasks": [5]})),
            ["the plan: sessions must be a list", "tasks[0]: must be an object"]
        );
    }

    #[test]
    fn parents_may_not_loop() {
        let fixture = Fixture::new();
        let problems = fixture.problems(json!({"sessions": [
            entry("a", json!({"parent": "b"})),
            entry("b", json!({"parent": "c"})),
            entry("c", json!({"parent": "a"})),
            entry("d", json!({"parent": "a"})),
        ]}));
        assert_eq!(
            problems,
            ["sessions[0] (a): parents go round in a loop (a → b → c → a)"]
        );
    }

    #[test]
    fn tasks_must_be_open_free_and_started_once() {
        let mut fixture = Fixture::new();
        let mut done = task("dd000001", "Done one");
        done.done = true;
        let mut taken = task("ee000001", "Taken one");
        taken.meta.wt = Some("wt-1".into());
        fixture.tasks.extend([done, taken, task("ab99", "Other")]);
        let problems = fixture.problems(json!({"sessions": [
            entry("a", json!({"task": "dd"})),
            entry("b", json!({"task": "ee"})),
            entry("c", json!({"task": "ab12cd34"})),
            entry("d", json!({"task": "ab12"})),
            entry("e", json!({"task": "ab"})),
        ]}));
        assert_eq!(
            problems,
            [
                "sessions[0] (a): task \"Done one\" is done",
                "sessions[1] (b): task \"Taken one\" is already being worked on",
                "sessions[3] (d): task \"Remove the legacy endpoint\" is already started by sessions[2]",
                "sessions[4] (e): task \"ab\" is the start of more than one task ID; give more of it",
            ]
        );
    }

    #[test]
    fn names_may_not_clash_with_active_sessions_or_each_other() {
        let fixture = Fixture::new();
        let problems = fixture.problems(json!({"sessions": [
            entry("a", json!({"title": "Fix login"})),
            entry("b", json!({"title": "Tests", "parent": "1f2e"})),
            entry("c", json!({"title": "Tests", "parent": "1f2e"})),
            // An exited session's name is free again.
            entry("d", json!({"title": "Old"})),
        ]}));
        assert_eq!(
            problems,
            [
                "sessions[0] (a): an active session named \"Fix login\" already exists in this repository",
                "sessions[2] (c): sessions[1] has the same name under the same parent (\"Fix login / Tests\")",
            ]
        );
    }

    #[test]
    fn an_ambiguous_parent_prefix_is_refused() {
        let mut fixture = Fixture::new();
        fixture.sessions.push(session("1f2e9999", "Other", "running"));
        let problems = fixture.problems(json!({"sessions": [entry("a", json!({"parent": "1f2e"}))]}));
        assert_eq!(
            problems,
            ["sessions[0] (a): parent \"1f2e\" is the start of more than one session ID; give more of it"]
        );
    }

    #[test]
    fn the_summary_shows_the_tree_and_how_each_session_stacks() {
        let fixture = Fixture::new();
        let valid = fixture.valid(json!({
            "sessions": [
                entry("auth", json!({"title": "Auth rework", "base": "main"})),
                entry("tokens", json!({"title": "Token storage", "parent": "auth"})),
                entry("ui", json!({"title": "Login UI", "parent": "auth", "worktree": "same", "agent": "codex"})),
                entry("deep", json!({"title": "Deep", "parent": "ui"})),
                entry("flat", json!({"title": "Flat", "parent": "auth", "flat": true})),
                entry("tests", json!({"title": "Tests", "parent": "1f2e", "task": "ab12"})),
                entry("other", json!({"title": "Other base", "parent": "1f2e", "base": "main"})),
                entry("script", json!({"title": "Migration script", "worktree": "none"})),
            ],
            "tasks": [{"title": "Remove legacy endpoint", "details": "After Auth rework merges"}],
        }));
        let stacked: Vec<(&str, u32, Option<bool>, Option<&str>)> = valid
            .summary
            .sessions
            .iter()
            .map(|entry| {
                (
                    entry.key.as_str(),
                    entry.depth,
                    entry.stacked,
                    entry.stacked_on.as_deref(),
                )
            })
            .collect();
        assert_eq!(
            stacked,
            [
                ("auth", 0, Some(false), None),
                ("tokens", 1, Some(true), None),
                ("ui", 1, None, None),
                // Under a session sharing its parent's new worktree: stacked on that worktree's branch.
                ("deep", 2, Some(true), None),
                ("flat", 1, Some(false), None),
                ("tests", 0, Some(true), Some("fix-login")),
                ("other", 0, Some(false), None),
                ("script", 0, None, None),
            ]
        );
        assert_eq!(valid.summary.counts, "8 sessions, 1 task");
        assert_eq!(
            summary_lines(&valid.summary),
            [
                "◇ Auth rework  (new worktree from main · claude)",
                "  ↳ Token storage  (stacked on Auth rework's branch · claude)",
                "  ↳ Login UI  (in its parent's worktree · codex)",
                "    ↳ Deep  (stacked on Auth rework's branch · claude)",
                "  ↳ Flat  (flat new worktree · claude)",
                "◇ Tests  (under Fix login · stacked on fix-login · claude · task: Remove the legacy endpoint)",
                "◇ Other base  (under Fix login · flat new worktree from main · claude)",
                "◇ Migration script  (no worktree · claude)",
                "☐ Backlog: Remove legacy endpoint",
                "8 sessions, 1 task",
            ]
        );
        let tests = &valid.summary.sessions[5];
        assert_eq!(tests.parent_title.as_deref(), Some("Fix login"));
        assert_eq!(tests.parent_session_id.as_deref(), Some("1f2e3d4c-lead"));
        assert_eq!(tests.task_title.as_deref(), Some("Remove the legacy endpoint"));
        assert_eq!(valid.summary.sessions[1].parent_key.as_deref(), Some("auth"));
        // The confirmation lays the same tree out in parts: indent, marker, title, then the rest muted.
        assert_eq!(
            entry_line(&valid.summary.sessions[3]),
            PlanEntryLine {
                indent: 4,
                marker: "↳",
                title: "Deep".to_owned(),
                detail: "stacked on Auth rework's branch · claude".to_owned(),
            }
        );
    }

    #[test]
    fn a_pending_plan_says_how_long_it_waits_in_whole_minutes() {
        let at = crate::jsstr::date::parse("2026-11-14T12:15:00.000Z").unwrap_or_default();
        let expiry = |minutes_left: f64| expiry_text("2026-11-14T12:15:00.000Z", at - (minutes_left * 60_000.0) as i64);
        assert_eq!(expiry(15.0).as_deref(), Some("expires in 15 min"));
        assert_eq!(expiry(14.5).as_deref(), Some("expires in 14 min"));
        assert_eq!(expiry(0.9).as_deref(), Some("expires in under a minute"));
        // Past it (the daemon settles it any moment): still under a minute, never negative.
        assert_eq!(expiry(-1.0).as_deref(), Some("expires in under a minute"));
        assert_eq!(expiry_text("not a date", at), None);
    }

    #[test]
    fn counts_leave_out_what_there_is_none_of() {
        assert_eq!(plan_counts(1, 0), "1 session");
        assert_eq!(plan_counts(0, 2), "2 tasks");
        assert_eq!(plan_counts(3, 1), "3 sessions, 1 task");
        assert_eq!(
            plan_problems_message(&["a".into(), "b".into()]),
            "The plan has 2 problems (nothing was created):\n- a\n- b"
        );
    }
}
