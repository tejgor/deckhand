//! Where sessions are on the list, as the session views say it: the `◆` line of a session's details (and what `/`
//! finds it by), what merging a worktree ticks and lifts, and what deleting one unhooks. Pure: the list
//! ([`TaskList`]) and the session records in; the daemon decides (`notes_tasks.rs`), this only tells beforehand.

use std::collections::HashSet;

use deckhand_protocol::data::SessionRecord;

use crate::session_order::SessionLike;
use crate::sidebar_model::DetailTask;
use crate::task_list::{Line, TaskList};

/// The sessions whose lines a change to worktree `record_id` touches: every session of that record. A session in a
/// worktree without a record ID (an old one) stands for itself.
pub fn worktree_session_ids<S: SessionLike>(session: &SessionRecord, sessions: &[S]) -> Vec<String> {
    let record = session
        .worktree
        .as_ref()
        .and_then(|worktree| worktree.id.as_deref())
        .filter(|id| !id.is_empty());
    let Some(record) = record else {
        return vec![session.id.clone()];
    };
    let mut ids: Vec<String> = sessions
        .iter()
        .map(SessionLike::session)
        .filter(|other| other.worktree.as_ref().and_then(|worktree| worktree.id.as_deref()) == Some(record))
        .map(|other| other.id.clone())
        .collect();
    if !ids.contains(&session.id) {
        ids.insert(0, session.id.clone());
    }
    ids
}

/// The `◆` line of a session's details: the title of the line it is on, whether it is ticked, and `ticked/total` of
/// the lines under it. `None` when it is on none (most quick sessions).
pub fn detail_line(list: &TaskList, session_id: &str) -> Option<DetailTask> {
    let line = list.line_of_session(session_id)?;
    let count = list.count(&line.id);
    Some(DetailTask {
        title: line.title.clone(),
        done: line.done,
        steps: (count.total > 0).then_some(count),
    })
}

/// The title of the line a session is on, which `/` searches.
pub fn line_title(list: &TaskList, session_id: &str) -> Option<String> {
    list.line_of_session(session_id).map(|line| line.title.clone())
}

/// The open lines `session_ids` are on, once each, in file order.
pub fn open_lines<'a>(list: &'a TaskList, session_ids: &[String]) -> Vec<&'a Line> {
    let ids: HashSet<&str> = session_ids.iter().map(String::as_str).collect();
    list.lines()
        .iter()
        .filter(|line| !line.done && line.meta.s.as_deref().is_some_and(|s| ids.contains(s)))
        .collect()
}

/// Where the open steps of ticked lines move up to: beside the ticked line, so under the line above it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LiftTarget {
    /// The ticked lines are top-level: their steps join the backlog.
    Backlog,
    /// They are all under this line.
    Line(String),
    /// Under different lines.
    Several,
}

impl LiftTarget {
    /// How the merge confirmation names it.
    pub fn phrase(&self) -> &str {
        match self {
            LiftTarget::Backlog => "the backlog",
            LiftTarget::Line(title) => title,
            LiftTarget::Several => "the lines above them",
        }
    }
}

/// What merging a worktree does to the list (`merge-worktree`): the lines its sessions are on tick, and the open
/// steps under them move up out of them unless kept (`keepOpenSteps`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MergeLines {
    /// The titles of the open lines that tick.
    pub ticks: Vec<String>,
    /// The open lines directly under them (not themselves ticking), which move with what is under them.
    pub open_steps: usize,
    pub to: LiftTarget,
}

/// What merging the worktree of `session_ids` (see [`worktree_session_ids`]) does to `list`.
pub fn merge_lines(list: &TaskList, session_ids: &[String]) -> MergeLines {
    let ticked = open_lines(list, session_ids);
    let ticking: HashSet<&str> = ticked.iter().map(|line| line.id.as_str()).collect();
    let mut open_steps = 0;
    let mut targets: Vec<Option<&str>> = Vec::new();
    for line in &ticked {
        let open = list
            .children(&line.id)
            .into_iter()
            .filter(|step| !step.done && !ticking.contains(step.id.as_str()))
            .count();
        if open == 0 {
            continue;
        }
        open_steps += open;
        let parent = line.parent.as_deref();
        if !targets.contains(&parent) {
            targets.push(parent);
        }
    }
    let to = match targets.as_slice() {
        [Some(parent)] => list
            .get(parent)
            .map_or(LiftTarget::Several, |line| LiftTarget::Line(line.title.clone())),
        [None] | [] => LiftTarget::Backlog,
        _ => LiftTarget::Several,
    };
    MergeLines {
        ticks: ticked.iter().map(|line| line.title.clone()).collect(),
        open_steps,
        to,
    }
}

/// `1 open step`, `2 open steps`.
pub fn open_steps_text(count: usize) -> String {
    format!("{count} open step{}", if count == 1 { "" } else { "s" })
}

/// What deleting a worktree unmerged does to the lines of its sessions (`titles`, from [`open_lines`]): they stay
/// where they are, nobody on them, marked tried. `None` without any.
pub fn tried_text(titles: &[String]) -> Option<String> {
    match titles {
        [] => None,
        [title] => Some(format!("◆ {title} goes back to the list marked tried")),
        _ => Some(format!(
            "◆ {} lines go back to the list marked tried: {}",
            titles.len(),
            titles.join(", ")
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task_list::LineCount;
    use serde_json::json;

    /// The walkthrough's idea: an orchestrator on it, Token storage with steps (one handed to a sub-session).
    const IDEA: &str = "\
- [ ] Rework auth <!-- dh:t=idea s=s-orch -->
  - [ ] Token storage <!-- dh:t=tok s=s-tok -->
    - [x] schema <!-- dh:t=sch -->
    - [ ] encryption <!-- dh:t=enc -->
    - [ ] refresh tokens <!-- dh:t=ref s=s-ref -->
  - [ ] Login UI <!-- dh:t=ui s=s-ui -->
- [ ] Ship it <!-- dh:t=ship s=s-ship -->
  - [ ] docs <!-- dh:t=docs -->
- [ ] Backlog item <!-- dh:t=b -->
";

    fn session(id: &str, worktree: Option<&str>) -> SessionRecord {
        let mut value = json!({
            "id": id, "title": id, "program": "claude", "command": "claude", "cwd": "/repo", "repoRoot": "/repo",
            "createdAt": "2026-11-14T10:00:00.000Z", "updatedAt": "2026-11-14T10:00:00.000Z", "status": "running",
        });
        if let Some(worktree) = worktree {
            value["worktree"] = json!({"mode": "managed", "path": format!("/wt/{worktree}"), "id": worktree});
        }
        serde_json::from_value(value).unwrap_or_else(|error| panic!("{error}"))
    }

    fn ids(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| (*item).to_owned()).collect()
    }

    #[test]
    fn the_details_show_the_sessions_line_and_its_steps() {
        let list = TaskList::parse(IDEA);
        assert_eq!(
            detail_line(&list, "s-tok"),
            Some(DetailTask {
                title: "Token storage".into(),
                done: false,
                steps: Some(LineCount { ticked: 1, total: 3 }),
            })
        );
        // A line without steps shows no count; a session on no line has no `◆` line.
        assert_eq!(detail_line(&list, "s-ui").and_then(|task| task.steps), None);
        assert_eq!(detail_line(&list, "s-none"), None);
        assert_eq!(line_title(&list, "s-ref").as_deref(), Some("refresh tokens"));
    }

    #[test]
    fn a_worktrees_sessions_are_every_session_of_its_record() {
        let a = session("a", Some("w1"));
        let b = session("b", Some("w1"));
        let other = session("c", Some("w2"));
        let sessions = vec![a.clone(), b, other];
        assert_eq!(worktree_session_ids(&a, &sessions), ids(&["a", "b"]));
        // No record ID: the session alone.
        assert_eq!(worktree_session_ids(&session("x", None), &sessions), ids(&["x"]));
    }

    #[test]
    fn merging_ticks_the_sessions_lines_and_lifts_their_open_steps_to_the_line_above() {
        let list = TaskList::parse(IDEA);
        assert_eq!(
            merge_lines(&list, &ids(&["s-tok"])),
            MergeLines {
                ticks: ids(&["Token storage"]),
                open_steps: 2,
                to: LiftTarget::Line("Rework auth".into()),
            }
        );
        // A sub-session in the same worktree ticks too, and its line is not one of the steps that move.
        assert_eq!(
            merge_lines(&list, &ids(&["s-tok", "s-ref"])),
            MergeLines {
                ticks: ids(&["Token storage", "refresh tokens"]),
                open_steps: 1,
                to: LiftTarget::Line("Rework auth".into()),
            }
        );
        // A top-level line's steps join the backlog; lines under different lines move under each.
        assert_eq!(merge_lines(&list, &ids(&["s-ship"])).to, LiftTarget::Backlog);
        assert_eq!(merge_lines(&list, &ids(&["s-ship", "s-tok"])).to, LiftTarget::Several);
        assert_eq!(LiftTarget::Several.phrase(), "the lines above them");
        // Nothing open under it: nothing moves.
        let quiet = merge_lines(&list, &ids(&["s-ui"]));
        assert_eq!((quiet.ticks.len(), quiet.open_steps), (1, 0));
        // A ticked line is not ticked again; no line, nothing.
        let ticked = TaskList::parse("- [x] Done <!-- dh:t=d s=s1 -->\n  - [ ] left <!-- dh:t=l -->\n");
        assert_eq!(
            merge_lines(&ticked, &ids(&["s1"])),
            MergeLines {
                ticks: Vec::new(),
                open_steps: 0,
                to: LiftTarget::Backlog,
            }
        );
    }

    #[test]
    fn deleting_unmerged_says_which_lines_are_marked_tried() {
        let list = TaskList::parse(IDEA);
        let titles: Vec<String> = open_lines(&list, &ids(&["s-ui", "s-ship"]))
            .iter()
            .map(|line| line.title.clone())
            .collect();
        assert_eq!(titles, ids(&["Login UI", "Ship it"]));
        assert_eq!(
            tried_text(&titles[..1]).as_deref(),
            Some("◆ Login UI goes back to the list marked tried")
        );
        assert_eq!(
            tried_text(&titles).as_deref(),
            Some("◆ 2 lines go back to the list marked tried: Login UI, Ship it")
        );
        assert_eq!(tried_text(&[]), None);
        assert_eq!(open_steps_text(1), "1 open step");
        assert_eq!(open_steps_text(3), "3 open steps");
    }
}
