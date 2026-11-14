//! Who works on what, as the board says it: the sessions of a work (keyed `wt:<record id>` / `s:<session id>`, see
//! `tasks::link_key`), the most urgent of them, and the header's count of open tasks. The board itself (the list as a
//! tree, its Notes view and its `w` menu) is `lines_board`.

use crate::session_order::SessionLike;
use crate::tasks::link_of_key;
use deckhand_protocol::data::{AgentActivityStatus, AttentionState, SessionRecord, SessionStatus, Task, TaskLink};

/// TS: tasksBoard.ts workSessions.
pub fn work_sessions<'a, S: SessionLike>(section: &str, sessions: &'a [S]) -> Vec<&'a S> {
    match link_of_key(section) {
        Some(TaskLink::Worktree { wt }) => sessions
            .iter()
            .filter(|item| {
                item.session()
                    .worktree
                    .as_ref()
                    .and_then(|worktree| worktree.id.as_deref())
                    == Some(wt.as_str())
            })
            .collect(),
        Some(TaskLink::Session { s }) => sessions.iter().filter(|item| item.session().id == s).collect(),
        None => Vec::new(),
    }
}

/// TS: tasksBoard.ts urgency (needs you 3, working 2, idle 1, exited 0).
pub fn urgency(session: &SessionRecord) -> u8 {
    let attention = if session.status == SessionStatus::Running {
        session.attention.as_ref().map(|attention| attention.state)
    } else {
        None
    };
    if matches!(
        attention,
        Some(AttentionState::NeedsInput | AttentionState::Failed | AttentionState::Limited)
    ) {
        return 3;
    }
    if matches!(session.status, SessionStatus::Running | SessionStatus::Starting) {
        return if session.agent_status == Some(AgentActivityStatus::Active) {
            2
        } else {
            1
        };
    }
    0
}

/// TS: tasksBoard.ts leadSession (the most urgent; the first on ties).
pub fn lead_session<S: SessionLike>(sessions: &[S]) -> Option<&S> {
    sessions.iter().fold(None, |best: Option<&S>, item| match best {
        Some(best) if urgency(item.session()) <= urgency(best.session()) => Some(best),
        _ => Some(item),
    })
}

/// TS: tasksBoard.ts taskCountLabel (`☐ 3 tasks · b`, empty without open tasks).
pub fn task_count_label(tasks: &[Task]) -> String {
    match tasks.iter().filter(|task| !task.done).count() {
        0 => String::new(),
        1 => "☐ 1 task · b".to_owned(),
        open => format!("☐ {open} tasks · b"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jsstr::date;
    use crate::session_features::tests::sessions_arg;
    use deckhand_testkit::{Case, replay, tags};
    use serde_json::Value;

    // ── Fixture plumbing ──────────────────────────────────────────────────────────────────────────────────────

    fn de<T: serde::de::DeserializeOwned>(value: Value) -> Result<T, String> {
        serde_json::from_value(value).map_err(|error| error.to_string())
    }

    fn tasks_arg(value: &Value) -> Result<Vec<Task>, String> {
        de(tags::untag(value))
    }

    /// Index of `item` (a reference into `list`).
    fn index_in<T>(list: &[T], item: &T) -> usize {
        list.iter()
            .position(|candidate| std::ptr::eq(candidate, item))
            .unwrap_or(usize::MAX)
    }

    /// The given argument item at `index` (TS returns the same objects it was given).
    fn given(arg: &Value, index: usize) -> Value {
        arg.get(index).cloned().unwrap_or(Value::Null)
    }

    // ── Ported TS tests (tests/tasksBoard.test.ts) ────────────────────────────────────────────────────────────

    fn now() -> i64 {
        date::parse("2026-10-09T12:00:00Z").unwrap_or_default()
    }

    // ── Parity replays ────────────────────────────────────────────────────────────────────────────────────────

    #[test]
    fn work_sessions_parity() {
        replay("tasksBoard/workSessions", |case: &Case| -> Result<Value, String> {
            let sessions = sessions_arg(case.arg(1))?;
            let found = work_sessions(case.arg_str(0)?, &sessions);
            Ok(Value::Array(
                found
                    .into_iter()
                    .map(|item| given(case.arg(1), index_in(&sessions, item)))
                    .collect(),
            ))
        });
    }

    #[test]
    fn lead_session_parity() {
        replay("tasksBoard/leadSession", |case: &Case| -> Result<Value, String> {
            let sessions = sessions_arg(case.arg(0))?;
            Ok(lead_session(&sessions)
                .map_or_else(tags::undefined, |lead| given(case.arg(0), index_in(&sessions, lead))))
        });
    }

    #[test]
    fn task_count_label_parity() {
        replay("tasksBoard/taskCountLabel", |case: &Case| {
            tasks_arg(case.arg(0)).map(|tasks| task_count_label(&tasks))
        });
    }

    #[test]
    fn an_added_line_says_where_it_was_left_open() -> Result<(), Box<dyn std::error::Error>> {
        use crate::tasks::apply_task_op;
        use deckhand_protocol::data::{TaskLink, TaskOp};
        let added = apply_task_op(
            "",
            &TaskOp::Add {
                title: "Write the parser".to_owned(),
                body: None,
                link: None,
                id: Some("id0".to_owned()),
                from: Some("feat/auth".to_owned()),
                parent: None,
                at: None,
            },
            now(),
        )?;
        assert!(
            added
                .text
                .starts_with("- [ ] Write the parser <!-- dh:t=id0 from=feat/auth added=2026-10-09 -->"),
            "{}",
            added.text
        );
        // Assigned to a work, `from` does not apply.
        let linked = apply_task_op(
            "",
            &TaskOp::Add {
                title: "x".to_owned(),
                body: None,
                link: Some(TaskLink::Worktree { wt: "w1".to_owned() }),
                id: None,
                from: Some("feat/auth".to_owned()),
                parent: None,
                at: None,
            },
            now(),
        )?;
        assert!(!linked.text.contains("from="), "{}", linked.text);
        Ok(())
    }
}
