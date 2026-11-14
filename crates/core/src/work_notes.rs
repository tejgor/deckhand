//! The notes around the list (`docs/future/work-items.md`, *Notes*), as pure text: a line's brief and its file, a
//! note's checkbox promoted to a line, what is still open in a session's note, and what a session started on a line
//! finds typed. The daemon reads and writes the files (`notes/lines/`, `notes/briefs/`, `notes/sessions/`,
//! `notes/repos/`).

use crate::jsstr::{self, is_js_whitespace};
use crate::notes::parse_checklist_line;
use crate::tasks::{clean_task_title, task_prompt};
use deckhand_protocol::data::{Task, TaskMeta};

/// What a brief's file holds: who wrote it, when, and its text.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Brief {
    pub from: Option<String>,
    pub at: Option<String>,
    pub text: String,
}

const BRIEF_OPEN: &str = "<!-- dh:brief";
const BRIEF_CLOSE: &str = "-->";

/// A value fit for the bookkeeping line: one line, no comment markers.
fn bookkeeping_value(value: &str) -> String {
    let one_line: String = value.chars().map(|ch| if ch.is_control() { ' ' } else { ch }).collect();
    jsstr::js_collapse_whitespace(&one_line.replace("<!--", "").replace("-->", ""))
        .trim()
        .to_owned()
}

/// A brief's file: `<!-- dh:brief at=<when> from=<who> -->` on its first line (`from` last: it may hold spaces), then
/// its text.
pub fn format_brief(brief: &Brief) -> String {
    let mut head = BRIEF_OPEN.to_owned();
    if let Some(at) = brief.at.as_deref().map(bookkeeping_value).filter(|at| !at.is_empty()) {
        let at: String = at.chars().filter(|ch| !is_js_whitespace(*ch)).collect();
        head.push_str(&format!(" at={at}"));
    }
    if let Some(from) = brief
        .from
        .as_deref()
        .map(bookkeeping_value)
        .filter(|from| !from.is_empty())
    {
        head.push_str(&format!(" from={from}"));
    }
    head.push_str(" -->");
    let text = jsstr::js_trim_end(&brief.text);
    if text.is_empty() {
        format!("{head}\n")
    } else {
        format!("{head}\n{text}\n")
    }
}

/// A brief's file read back. A file without the bookkeeping line (written by hand) is all text.
pub fn parse_brief(file: &str) -> Brief {
    let (first, rest) = file.split_once('\n').unwrap_or((file, ""));
    let head = first.trim_end_matches('\r').trim();
    let Some(inner) = head
        .strip_prefix(BRIEF_OPEN)
        .and_then(|inner| inner.strip_suffix(BRIEF_CLOSE))
    else {
        return Brief {
            text: file.to_owned(),
            ..Brief::default()
        };
    };
    let inner = inner.trim();
    let (pairs, from) = match inner.find("from=") {
        Some(at) if at == 0 || inner[..at].ends_with(' ') => (&inner[..at], Some(inner[at + 5..].trim().to_owned())),
        _ => (inner, None),
    };
    let at = pairs
        .split_whitespace()
        .find_map(|pair| pair.strip_prefix("at="))
        .filter(|at| !at.is_empty())
        .map(str::to_owned);
    Brief {
        from: from.filter(|from| !from.is_empty()),
        at,
        text: rest.to_owned(),
    }
}

fn indent_col(line: &str) -> usize {
    let mut col = 0;
    for ch in line.chars() {
        match ch {
            ' ' => col += 1,
            '\t' => col = (col / 4 + 1) * 4,
            _ => break,
        }
    }
    col
}

fn is_blank(line: &str) -> bool {
    jsstr::js_trim(line).is_empty()
}

/// How many lines after `at` belong to the line at `at`: the run of lines indented more than it (a blank line inside
/// the run belongs to it only when an indented line follows).
fn owned_after(lines: &[&str], at: usize) -> usize {
    let Some(own) = lines.get(at).map(|line| indent_col(line)) else {
        return 0;
    };
    let mut count = 0;
    let mut confirmed = 0;
    for line in lines.iter().skip(at + 1) {
        if is_blank(line) {
            count += 1;
            continue;
        }
        if indent_col(line) <= own {
            break;
        }
        count += 1;
        confirmed = count;
    }
    confirmed
}

/// The lines with their common indent removed.
fn dedent(lines: &[&str]) -> String {
    let cut = lines
        .iter()
        .filter(|line| !is_blank(line))
        .map(|line| line.len() - line.trim_start_matches([' ', '\t']).len())
        .min()
        .unwrap_or(0);
    lines
        .iter()
        .map(|line| {
            let own = line.len() - line.trim_start_matches([' ', '\t']).len();
            &line[cut.min(own)..]
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A checkbox taken out of a note to become a line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TakenCheckbox {
    /// The new line's title.
    pub title: String,
    pub done: bool,
    /// The text indented under the checkbox (its nested checkboxes included, as text): the new line's note.
    pub note: String,
    /// The note without the checkbox and what is indented under it.
    pub text: String,
}

/// The checkbox on `line` (0-based) of a note, taken out with what is indented under it (`ctrl+p`); `None` unless that
/// line is a checklist item with a title.
pub fn take_checkbox(note: &str, line: usize) -> Option<TakenCheckbox> {
    let lines: Vec<&str> = note.split('\n').collect();
    let item = parse_checklist_line(lines.get(line)?)?;
    let title = clean_task_title(&item.text);
    if title.is_empty() {
        return None;
    }
    let owned = owned_after(&lines, line);
    let under: Vec<&str> = lines.iter().skip(line + 1).take(owned).copied().collect();
    let kept: Vec<&str> = lines
        .iter()
        .enumerate()
        .filter(|(at, _)| *at < line || *at > line + owned)
        .map(|(_, text)| *text)
        .collect();
    Some(TakenCheckbox {
        title,
        done: item.checked,
        note: dedent(&under).trim_matches('\n').to_owned(),
        text: kept.join("\n"),
    })
}

/// What is still open in a note: all of it but its ticked checkboxes (with what is indented under them) and `↗` lines
/// of the old model, blank runs closed up; empty when nothing else is left.
pub fn open_note_text(note: &str) -> String {
    let lines: Vec<&str> = note.split('\n').collect();
    let mut kept: Vec<&str> = Vec::new();
    let mut at = 0;
    while at < lines.len() {
        let line = lines[at];
        if parse_checklist_line(line).is_some_and(|item| item.checked)
            || crate::tasks::parse_note_task_link(line).is_some()
        {
            at += 1 + owned_after(&lines, at);
            continue;
        }
        if !(is_blank(line) && kept.last().is_none_or(|last| is_blank(last))) {
            kept.push(line);
        }
        at += 1;
    }
    while kept.last().is_some_and(|last| is_blank(last)) {
        kept.pop();
    }
    kept.join("\n")
}

/// `addition` appended to a note as a paragraph of its own (after a blank line), ending in a newline.
pub fn append_paragraph(note: &str, addition: &str) -> String {
    let base = jsstr::js_trim_end(note);
    let addition = jsstr::js_trim(addition);
    match (base.is_empty(), addition.is_empty()) {
        (_, true) => note.to_owned(),
        (true, false) => format!("{addition}\n"),
        (false, false) => format!("{base}\n\n{addition}\n"),
    }
}

/// What a session started on a line finds typed into its agent's input: the line's title, then its brief and its
/// note, on one line (as a task's text always was).
pub fn line_prompt(title: &str, brief: &str, note: &str) -> String {
    let body: Vec<&str> = [brief, note]
        .into_iter()
        .map(jsstr::js_trim)
        .filter(|text| !text.is_empty())
        .collect();
    task_prompt(&Task {
        id: String::new(),
        title: title.to_owned(),
        done: false,
        body: body.join("\n"),
        meta: TaskMeta::default(),
    })
}

/// The title of the line a session gets when something needs one: its own name (not its parents'), else `Session`.
pub fn session_line_title(session_title: &str) -> String {
    let title = clean_task_title(session_title);
    if title.is_empty() { "Session".to_owned() } else { title }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_brief_keeps_who_wrote_it_and_when_on_its_first_line() {
        let brief = Brief {
            from: Some("◎ Orchestrator".into()),
            at: Some("2026-11-14T12:00:00.000Z".into()),
            text: "Store tokens encrypted.\n\nDone when the tests pass.\n\n".into(),
        };
        let file = format_brief(&brief);
        assert_eq!(
            file,
            "<!-- dh:brief at=2026-11-14T12:00:00.000Z from=◎ Orchestrator -->\nStore tokens encrypted.\n\nDone when \
             the tests pass.\n"
        );
        assert_eq!(
            parse_brief(&file),
            Brief {
                text: "Store tokens encrypted.\n\nDone when the tests pass.\n".into(),
                ..brief
            }
        );
    }

    #[test]
    fn a_brief_written_by_hand_is_all_text_and_markers_never_break_its_line() {
        assert_eq!(
            parse_brief("Just a brief\n"),
            Brief {
                text: "Just a brief\n".into(),
                ..Brief::default()
            }
        );
        let file = format_brief(&Brief {
            from: Some("you -->\nme".into()),
            at: None,
            text: String::new(),
        });
        assert_eq!(file, "<!-- dh:brief from=you me -->\n");
        assert_eq!(parse_brief(&file).from.as_deref(), Some("you me"));
        assert_eq!(parse_brief(&file).at, None);
    }

    #[test]
    fn a_promoted_checkbox_takes_what_is_indented_under_it_as_its_note() {
        let note = "Thinking\n- [ ] Retry setup\n  when trust changes\n  - [ ] and log it\n\n- [x] Done thing\nend";
        let taken = take_checkbox(note, 1).unwrap();
        assert_eq!(taken.title, "Retry setup");
        assert!(!taken.done);
        assert_eq!(taken.note, "when trust changes\n- [ ] and log it");
        assert_eq!(taken.text, "Thinking\n\n- [x] Done thing\nend");
        assert_eq!(
            take_checkbox(note, 5).map(|taken| (taken.title, taken.done)),
            Some(("Done thing".into(), true))
        );
        assert_eq!(take_checkbox(note, 0), None);
        assert_eq!(take_checkbox(note, 99), None);
        assert_eq!(take_checkbox("- [ ] ", 0), None);
    }

    #[test]
    fn the_open_text_of_a_note_leaves_out_ticked_checkboxes() {
        let note = "Review crypto.rs before merging\n\n- [x] told it about scope\n  details\n\n- [ ] check the \
                    diff\n- ↗ sent <!-- dh:t=abcd1234 -->\n";
        assert_eq!(
            open_note_text(note),
            "Review crypto.rs before merging\n\n- [ ] check the diff"
        );
        assert_eq!(open_note_text("- [x] all done\n\n"), "");
    }

    #[test]
    fn appended_text_is_a_paragraph_of_its_own() {
        assert_eq!(append_paragraph("", "Found it"), "Found it\n");
        assert_eq!(append_paragraph("Before\n\n", "  Found it \n"), "Before\n\nFound it\n");
        assert_eq!(append_paragraph("Before", " "), "Before");
    }

    #[test]
    fn a_session_on_a_line_gets_its_title_brief_and_note_on_one_line() {
        assert_eq!(
            line_prompt("Token storage", "Store them encrypted.\n", "\n- found the cache\n"),
            "Token storage: Store them encrypted. - found the cache"
        );
        assert_eq!(line_prompt("Token storage", "", ""), "Token storage");
        assert_eq!(session_line_title("  "), "Session");
        assert_eq!(session_line_title("Fix\nlogin"), "Fix login");
    }
}
