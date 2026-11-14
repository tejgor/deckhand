//! `src/tasks.ts` `parseTasks`, ported locally for black-box assertions on a repository's tasks file, and
//! [`parse_lines`], the nested list as Deckhand writes it (two spaces per level).

use std::process::{Command, Stdio};

use serde_json::Value;

#[derive(Debug, Clone)]
pub struct Task {
    pub id: String,
    pub title: String,
    pub done: bool,
    pub body: String,
    pub meta: std::collections::BTreeMap<String, String>,
}

pub const META_KEYS: [&str; 8] = ["t", "wt", "s", "done", "auto", "tried", "from", "added"];

/// `^([ \t]*)([-*+])([ \t]+)\[([ xX])\](?:[ \t]|$)`: (indent, checked, text after the marker).
pub fn checklist_line(line: &str) -> Option<(&str, bool, &str)> {
    let rest = line.trim_start_matches([' ', '\t']);
    let indent = &line[..line.len() - rest.len()];
    let mut chars = rest.chars();
    if !matches!(chars.next()?, '-' | '*' | '+') {
        return None;
    }
    let after = &rest[1..];
    let body = after.trim_start_matches([' ', '\t']);
    if body.len() == after.len() {
        return None;
    }
    let mut chars = body.chars();
    if chars.next()? != '[' {
        return None;
    }
    let mark = chars.next()?;
    if !matches!(mark, ' ' | 'x' | 'X') || chars.next()? != ']' {
        return None;
    }
    let tail = &body[3..];
    if !(tail.is_empty() || tail.starts_with([' ', '\t'])) {
        return None;
    }
    let text = if tail.is_empty() { tail } else { &tail[1..] };
    Some((indent, mark != ' ', text))
}

/// `[ \t]*<!--[ \t]*dh:([^>]*?)[ \t]*-->[ \t]*$`: (index where the match starts, captured pairs).
pub fn task_meta(text: &str) -> Option<(usize, &str)> {
    let trimmed = text.trim_end_matches([' ', '\t']);
    let inner_end = trimmed.strip_suffix("-->")?.len();
    let mut from = 0;
    while let Some(found) = trimmed[from..].find("<!--") {
        let start = from + found;
        let inner = trimmed[start + 4..inner_end].trim_start_matches([' ', '\t']);
        if let Some(captured) = inner.strip_prefix("dh:")
            && !captured.contains('>')
        {
            let begin = trimmed[..start].trim_end_matches([' ', '\t']).len();
            return Some((begin, captured.trim_end_matches([' ', '\t'])));
        }
        from = start + 4;
    }
    None
}

pub fn sha8(text: &str) -> String {
    sha_hex(text)[..8].to_string()
}

/// A repository's list key: the first 16 hex digits of the SHA-256 of its main checkout's path.
pub fn repo_key(root: &str) -> String {
    sha_hex(root)[..16].to_string()
}

/// The SHA-256 of `text`, in hex.
fn sha_hex(text: &str) -> String {
    use std::io::Write;
    let tool = if Command::new("shasum").arg("--version").output().is_ok() {
        ("shasum", vec!["-a", "256"])
    } else {
        ("sha256sum", vec![])
    };
    let mut child = Command::new(tool.0)
        .args(tool.1)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("sha256 tool");
    child.stdin.take().unwrap().write_all(text.as_bytes()).unwrap();
    let output = child.wait_with_output().unwrap();
    String::from_utf8_lossy(&output.stdout)[..64].to_string()
}

pub fn parse_tasks(text: &str) -> Vec<Task> {
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    let mut lines: Vec<&str> = text.split('\n').collect();
    if lines.last() == Some(&"") {
        lines.pop();
    }
    let mut tasks = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        let line = lines[index];
        index += 1;
        let Some((indent, checked, item_text)) = checklist_line(line) else {
            continue;
        };
        if !indent.is_empty() {
            continue;
        }
        let (title, meta_raw) = match task_meta(item_text) {
            Some((begin, captured)) => (item_text[..begin].trim().to_string(), Some(captured)),
            None => (item_text.trim().to_string(), None),
        };
        let mut body: Vec<&str> = Vec::new();
        while index < lines.len() && lines[index].starts_with([' ', '\t']) {
            body.push(lines[index]);
            index += 1;
        }
        let mut meta = std::collections::BTreeMap::new();
        for pair in meta_raw.unwrap_or("").split([' ', '\t']) {
            let Some(at) = pair.find('=').filter(|at| *at >= 1) else {
                continue;
            };
            let (key, value) = (&pair[..at], &pair[at + 1..]);
            if value.is_empty() || !META_KEYS.contains(&key) {
                continue;
            }
            if key == "auto" && !matches!(value, "merge" | "done" | "lines") {
                continue;
            }
            meta.insert(key.to_string(), value.to_string());
        }
        let indents: Vec<usize> = body
            .iter()
            .filter(|l| !l.trim().is_empty())
            .map(|l| l.len() - l.trim_start_matches([' ', '\t']).len())
            .collect();
        let cut = indents.iter().min().copied().unwrap_or(0);
        let dedented: Vec<&str> = body
            .iter()
            .map(|l| {
                let own = l.len() - l.trim_start_matches([' ', '\t']).len();
                &l[cut.min(own)..]
            })
            .collect();
        let position = tasks.len();
        tasks.push(Task {
            id: meta
                .get("t")
                .cloned()
                .unwrap_or_else(|| format!("~{position}:{}", sha8(&title))),
            title,
            done: checked,
            body: dedented.join("\n"),
            meta,
        });
    }
    tasks
}

pub fn meta_of(task: &Task, key: &str) -> Option<String> {
    task.meta.get(key).cloned()
}

/// The tasks of a `tasks` document as the daemon returns it (`{text}`).
pub fn tasks_of(doc: &Value) -> Vec<Task> {
    parse_tasks(doc["text"].as_str().unwrap())
}

/// One line of the nested list.
#[derive(Debug, Clone)]
pub struct ListLine {
    pub id: String,
    pub title: String,
    pub done: bool,
    /// Its indent in columns (Deckhand writes two per level).
    pub indent: usize,
    /// The ID of the line it is under.
    pub parent: Option<String>,
    pub meta: std::collections::BTreeMap<String, String>,
}

impl ListLine {
    pub fn meta(&self, key: &str) -> Option<&str> {
        self.meta.get(key).map(String::as_str)
    }
}

/// Every checklist line of the list that is a top-level line or under one (indented checklist items inside a
/// top-level line's block), with its parent: the nearest line above it indented less.
pub fn parse_lines(text: &str) -> Vec<ListLine> {
    let text = text.replace("\r\n", "\n");
    let mut out: Vec<ListLine> = Vec::new();
    let mut in_block = false;
    for line in text.split('\n') {
        let Some((indent, checked, item_text)) = checklist_line(line) else {
            if !line.starts_with([' ', '\t']) || line.trim().is_empty() {
                in_block = in_block && line.starts_with([' ', '\t']);
            }
            continue;
        };
        if !indent.is_empty() && !in_block {
            continue;
        }
        in_block = true;
        let (title, meta_raw) = match task_meta(item_text) {
            Some((begin, captured)) => (item_text[..begin].trim().to_string(), Some(captured)),
            None => (item_text.trim().to_string(), None),
        };
        let mut meta = std::collections::BTreeMap::new();
        for pair in meta_raw.unwrap_or("").split([' ', '\t']) {
            if let Some(at) = pair.find('=').filter(|at| *at >= 1)
                && at + 1 < pair.len()
            {
                meta.insert(pair[..at].to_string(), pair[at + 1..].to_string());
            }
        }
        let width = indent.len();
        let parent = out
            .iter()
            .rev()
            .take_while(|_| width > 0)
            .find(|above| above.indent < width)
            .map(|above| above.id.clone());
        out.push(ListLine {
            id: meta.get("t").cloned().unwrap_or_default(),
            title,
            done: checked,
            indent: width,
            parent,
            meta,
        });
    }
    out
}

/// The lines of a `tasks` document as the daemon returns it (`{text}`).
pub fn lines_of(doc: &serde_json::Value) -> Vec<ListLine> {
    parse_lines(doc["text"].as_str().unwrap_or_default())
}

/// The line titled `title`.
pub fn line_titled(lines: &[ListLine], title: &str) -> Option<ListLine> {
    lines.iter().find(|line| line.title == title).cloned()
}

/// The titles of the lines directly under line `id`.
pub fn titles_under(lines: &[ListLine], id: &str) -> Vec<String> {
    lines
        .iter()
        .filter(|line| line.parent.as_deref() == Some(id))
        .map(|line| line.title.clone())
        .collect()
}
