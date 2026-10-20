//! `src/tasks.ts` `parseTasks`, ported locally for black-box assertions on a repository's tasks file.

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
    String::from_utf8_lossy(&output.stdout)[..8].to_string()
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
            if key == "auto" && !matches!(value, "merge" | "done") {
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
