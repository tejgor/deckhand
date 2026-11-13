//! The resume checklist (confirm-resume): sessions to resume in one go, each on its own row with what it is (or was)
//! doing. Two offers open it:
//!
//! - **Interrupted**: once connected, the sessions that were live when the daemon last stopped (`deckhand restart`, a
//!   crash, a reboot), every one ticked. Leaving it stops offering them; `s` still resumes each.
//! - **Outdated**: on the Agents screen, after an update (or `s` there), the agent's running sessions on an older
//!   version than the one now installed. One that is working or waiting for you is not ticked: restarting it ends what
//!   it is doing.
//!
//! Enter asks the daemon to resume the ticked ones (`restart-sessions`), stopping a running one first. Both lists
//! cover every repository the daemon holds sessions for, so each row names its repository.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::Duration;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use deckhand_client::live::LiveClient;
use deckhand_core::agent_signals::live_activity;
use deckhand_core::agent_versions::{installed_versions, session_outdated};
use deckhand_core::agents::agent_spec;
use deckhand_core::display::{HintPart, RowStatus, error_message, fit_hint, session_path_title_by};
use deckhand_core::jsstr::{date, path};
use deckhand_core::key::Input;
use deckhand_protocol::data::{InterruptedWhile, ProgramKey, SessionRecord, SessionStatus};
use deckhand_sys::async_util::{Task, spawn};

use crate::app::state::{App, Mode};
use crate::keymap::{Binding, HelpRef, KeySpec, Keymap, NamedKey};
use crate::panes::menu::{SelectableRowProps, selectable_row};
use crate::primitives::{
    Border, DialogRows, DialogScroll, DialogView, Row, Seg, TextStyle, Wrap, draw_dialog, fit_row,
};
use crate::theme::THEME;

/// confirm-resume keys.
pub static KEYMAP: Keymap = &[
    Binding {
        keys: &[KeySpec::Named(NamedKey::Up), KeySpec::Char("k")],
        action: "previous session",
        help: HelpRef::Hidden("the checklist shows its own hint line"),
    },
    Binding {
        keys: &[KeySpec::Named(NamedKey::Down), KeySpec::Char("j")],
        action: "next session",
        help: HelpRef::Hidden("the checklist shows its own hint line"),
    },
    Binding {
        keys: &[KeySpec::Char(" ")],
        action: "tick or untick the session",
        help: HelpRef::Hidden("the checklist shows its own hint line"),
    },
    Binding {
        keys: &[KeySpec::Char("a")],
        action: "tick every session (or none, when all are)",
        help: HelpRef::Hidden("the checklist shows its own hint line"),
    },
    Binding {
        keys: &[KeySpec::Named(NamedKey::Return)],
        action: "resume the ticked sessions",
        help: HelpRef::Hidden("the checklist shows its own hint line"),
    },
    Binding {
        keys: &[KeySpec::Named(NamedKey::Escape)],
        action: "leave them as they are",
        help: HelpRef::Hidden("the checklist shows its own hint line"),
    },
    crate::keymap::SCROLL_DIALOG,
];

/// Which sessions the checklist offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResumeKind {
    /// The sessions the last daemon stop interrupted.
    Interrupted,
    /// The agent's running sessions on an older version than the one installed.
    Outdated(ProgramKey),
}

/// One session in the checklist.
#[derive(Clone, Debug, PartialEq)]
pub struct ResumeRow {
    pub session: SessionRecord,
    /// What it was doing when interrupted, or is doing now.
    pub activity: InterruptedWhile,
    pub ticked: bool,
}

/// confirm-resume mode's state.
#[derive(Debug, Default)]
pub struct ResumeState {
    /// `None` while the checklist is closed.
    pub kind: Option<ResumeKind>,
    pub rows: Vec<ResumeRow>,
    pub cursor: usize,
    /// The mode to go back to.
    pub back: Mode,
}

/// The exited sessions the daemon offers for resuming, all ticked, grouped by repository (in the daemon's order).
pub fn interrupted_rows(sessions: Vec<SessionRecord>) -> Vec<ResumeRow> {
    grouped(
        sessions
            .into_iter()
            .filter(|session| session.status == SessionStatus::Exited)
            .filter_map(|session| {
                let activity = session.interrupted_while?;
                Some(ResumeRow {
                    session,
                    activity,
                    ticked: true,
                })
            })
            .collect(),
    )
}

/// `program`'s live sessions on an older version than `installed`, grouped by repository; the idle ones ticked.
pub fn outdated_rows(sessions: Vec<SessionRecord>, program: ProgramKey, installed: Option<&str>) -> Vec<ResumeRow> {
    grouped(
        sessions
            .into_iter()
            .filter(|session| {
                session.program == program
                    && session.status != SessionStatus::Exited
                    && session_outdated(session, installed)
            })
            .map(|session| {
                let activity = live_activity(&session);
                ResumeRow {
                    session,
                    activity,
                    ticked: activity == InterruptedWhile::Idle,
                }
            })
            .collect(),
    )
}

/// `rows` with each repository's together, in the order the repositories first appear.
fn grouped(rows: Vec<ResumeRow>) -> Vec<ResumeRow> {
    let mut first_seen: HashMap<String, usize> = HashMap::new();
    for row in &rows {
        let next = first_seen.len();
        first_seen.entry(row.session.repo_root.clone()).or_insert(next);
    }
    let mut rows = rows;
    rows.sort_by_key(|row| first_seen[&row.session.repo_root]);
    rows
}

/// Once connected: asks the daemon for every session it offers for resuming, and opens the checklist when there are
/// any and the session list still has the keys.
pub fn offer_interrupted(app: &mut App) {
    let Some(client) = app.client() else { return };
    drop(app.dispatch().spawn(client.list_sessions(), |app: &mut App, result| {
        let Ok(sessions) = result else { return };
        let rows = interrupted_rows(sessions);
        if !rows.is_empty() && app.mode == Mode::Browse && app.resume.kind.is_none() {
            open(app, ResumeKind::Interrupted, rows);
        }
    }));
}

/// The Agents screen: offers to restart `program`'s sessions on an older version (after an update, or `s`). On
/// another screen by the time the list arrives, it only says so. `asked`: `s` asked for it, so an empty list is said.
pub fn offer_outdated(app: &mut App, program: ProgramKey, asked: bool) {
    let Some(client) = app.client() else { return };
    drop(
        app.dispatch()
            .spawn(client.list_sessions(), move |app: &mut App, result| {
                let sessions = match result {
                    Ok(sessions) => sessions,
                    Err(error) => {
                        app.set_error(Some(error_message(&error)));
                        return;
                    }
                };
                let installed = installed_versions(app.data.agent_versions.as_ref())
                    .get(&program)
                    .cloned();
                let rows = outdated_rows(sessions, program, installed.as_deref());
                let label = agent_spec(program).label;
                if rows.is_empty() {
                    if asked {
                        app.set_status(Some(format!("No running {label} session is on an older version")));
                    }
                    return;
                }
                if app.mode == Mode::Agents && app.resume.kind.is_none() {
                    open(app, ResumeKind::Outdated(program), rows);
                } else {
                    app.set_status(Some(format!(
                        "{} still on an older {label} · G, then s, restarts them",
                        sessions_word(rows.len())
                    )));
                }
            }),
    );
}

/// `1 session`, `2 sessions`.
fn sessions_word(count: usize) -> String {
    format!("{count} session{}", if count == 1 { "" } else { "s" })
}

fn open(app: &mut App, kind: ResumeKind, rows: Vec<ResumeRow>) {
    app.resume = ResumeState {
        kind: Some(kind),
        rows,
        cursor: 0,
        back: app.mode,
    };
    app.set_mode(Mode::ConfirmResume);
}

/// confirm-resume's keys.
pub fn handle_confirm_resume(app: &mut App, input: &Input) {
    let key = &input.key;
    let count = app.resume.rows.len().max(1);
    let cursor = app.resume.cursor.min(count - 1);
    if key.escape {
        close(app, &[]);
        return;
    }
    if key.up_arrow || input.input == "k" {
        app.resume.cursor = (cursor + count - 1) % count;
        return;
    }
    if key.down_arrow || input.input == "j" {
        app.resume.cursor = (cursor + 1) % count;
        return;
    }
    if input.input == " " {
        if let Some(row) = app.resume.rows.get_mut(cursor) {
            row.ticked = !row.ticked;
        }
        return;
    }
    if input.input == "a" {
        let all = app.resume.rows.iter().all(|row| row.ticked);
        for row in &mut app.resume.rows {
            row.ticked = !all;
        }
        return;
    }
    if key.return_ {
        let ticked: Vec<SessionRecord> = app
            .resume
            .rows
            .iter()
            .filter(|row| row.ticked)
            .map(|row| row.session.clone())
            .collect();
        let kind = app.resume.kind;
        close(app, &ticked);
        if let Some(kind) = kind
            && !ticked.is_empty()
        {
            resume(app, kind, &ticked);
        }
    }
}

/// Back to the mode it opened from. The interrupted sessions not being resumed are no longer offered.
fn close(app: &mut App, resuming: &[SessionRecord]) {
    let state = std::mem::take(&mut app.resume);
    app.set_mode(state.back);
    if state.kind != Some(ResumeKind::Interrupted) {
        return;
    }
    let dismissed: Vec<String> = state
        .rows
        .iter()
        .filter(|row| !resuming.iter().any(|session| session.id == row.session.id))
        .map(|row| row.session.id.clone())
        .collect();
    if let Some(client) = app.client()
        && !dismissed.is_empty()
    {
        // Otherwise the next connection would offer them again.
        drop(
            app.dispatch()
                .spawn(client.dismiss_interrupted(&dismissed), |_: &mut App, _| {}),
        );
    }
}

/// Asks the daemon to resume `sessions` (stopping running ones first) and says how that went.
fn resume(app: &mut App, kind: ResumeKind, sessions: &[SessionRecord]) {
    let Some(client) = app.client() else { return };
    let ids: Vec<String> = sessions.iter().map(|session| session.id.clone()).collect();
    let names: HashMap<String, String> = sessions
        .iter()
        .map(|session| (session.id.clone(), session_name(session)))
        .collect();
    let verb = match kind {
        ResumeKind::Interrupted => "Resuming",
        ResumeKind::Outdated(_) => "Restarting",
    };
    // Not busy: the daemon boots a few at a time, so this can take a while; the list shows them coming back.
    app.set_error(None);
    app.set_status(Some(format!("{verb} {}…", sessions_word(ids.len()))));
    let progress = count_launches(app, &client, &ids, verb);
    let layout = app.layout();
    let reply = client.restart_sessions(&ids, i64::from(layout.preview_cols), i64::from(layout.preview_rows));
    drop(app.dispatch().spawn(reply, move |app: &mut App, result| {
        progress.abort();
        let outcomes = match result {
            Ok(outcomes) => outcomes,
            Err(error) => {
                app.set_status(None);
                app.set_error(Some(error_message(&error)));
                return;
            }
        };
        let failed: Vec<String> = outcomes
            .iter()
            .filter_map(|outcome| {
                let error = outcome.error.as_deref()?;
                let name = names
                    .get(&outcome.session_id)
                    .map_or(outcome.session_id.as_str(), String::as_str);
                Some(format!("“{name}”: {error}"))
            })
            .collect();
        let done = outcomes.len() - failed.len();
        let past = match kind {
            ResumeKind::Interrupted => "Resumed",
            ResumeKind::Outdated(_) => "Restarted",
        };
        if failed.is_empty() {
            app.set_status(Some(format!("{past} {}", sessions_word(done))));
        } else {
            app.set_status(None);
            app.set_error(Some(format!(
                "{past} {} of {}; not {}",
                done,
                outcomes.len(),
                failed.join("; ")
            )));
        }
    }));
}

/// How often the status line counts the sessions launched so far.
const PROGRESS_EVERY: Duration = Duration::from_secs(1);

/// While the daemon resumes `ids`, says how many have launched so far (`Resuming 7 of 23…`): each one whose agent
/// started after now. Stopped (aborted) once the daemon answers.
fn count_launches(app: &App, client: &Rc<LiveClient>, ids: &[String], verb: &'static str) -> Task<()> {
    let since = date::to_iso_string(date::now_ms()).unwrap_or_default();
    let dispatch = app.dispatch().clone();
    let client = Rc::clone(client);
    let ids: HashSet<String> = ids.iter().cloned().collect();
    spawn(async move {
        let mut shown = 0;
        loop {
            tokio::time::sleep(PROGRESS_EVERY).await;
            let Ok(sessions) = client.list_sessions().await else {
                continue;
            };
            let launched = sessions
                .iter()
                .filter(|session| {
                    ids.contains(&session.id)
                        && session
                            .agent_started_at
                            .as_deref()
                            .is_some_and(|at| at > since.as_str())
                })
                .count();
            if launched != shown {
                shown = launched;
                let total = ids.len();
                dispatch.update(move |app: &mut App| {
                    app.set_status(Some(format!("{verb} {launched} of {total}…")));
                });
            }
        }
    })
}

/// A session's name in the list: its path (`parent / child`), as the daemon projected it.
fn session_name(session: &SessionRecord) -> String {
    session_path_title_by(session, |_| None)
}

/// What a row says the session is (or was) doing.
fn activity_text(kind: ResumeKind, activity: InterruptedWhile) -> &'static str {
    match (kind, activity) {
        (ResumeKind::Interrupted, InterruptedWhile::Working) => "was working",
        (ResumeKind::Interrupted, InterruptedWhile::NeedsInput) => "was waiting for you",
        (ResumeKind::Interrupted, InterruptedWhile::Idle) => "was idle",
        (ResumeKind::Outdated(_), InterruptedWhile::Working) => "working",
        (ResumeKind::Outdated(_), InterruptedWhile::NeedsInput) => "waiting for you",
        (ResumeKind::Outdated(_), InterruptedWhile::Idle) => "idle",
    }
}

/// The checklist's props.
#[derive(Clone, Copy, Debug)]
pub struct ResumeDialogProps<'a> {
    pub kind: ResumeKind,
    pub rows: &'a [ResumeRow],
    pub cursor: usize,
    /// The installed version of an Outdated list's agent.
    pub installed: Option<&'a str>,
}

fn text(text: impl Into<String>, style: TextStyle) -> Row {
    vec![Seg::new(text, style)]
}

/// The checklist as a dialog `width` wide.
pub fn resume_dialog_rows(props: &ResumeDialogProps<'_>, width: u16) -> DialogRows {
    let content_width = usize::from(width).saturating_sub(4).max(1);
    let mut rows = DialogRows::default();
    let (title, explanation, action) = match props.kind {
        ResumeKind::Interrupted => (
            "Resume the sessions the daemon stopped?".to_owned(),
            "They were running when Deckhand's daemon stopped. Each reopens its conversation and waits for you: one \
             that was working does not carry on by itself.",
            "resume",
        ),
        ResumeKind::Outdated(program) => (
            format!(
                "Restart {} sessions on {}?",
                agent_spec(program).label,
                props.installed.unwrap_or("the new version")
            ),
            "Each is stopped, then resumes its conversation on the new version. One that is working or waiting for \
             you would lose that, so it is not ticked.",
            "restart",
        ),
    };
    rows.header.extend(fit_row(
        &text(title, TextStyle::fg(THEME.accent).bold(true)),
        content_width,
        Wrap::Wrap,
    ));
    rows.body.push(Vec::new());
    rows.body.extend(fit_row(
        &text(explanation, TextStyle::fg(THEME.muted)),
        content_width,
        Wrap::Wrap,
    ));
    rows.body.push(Vec::new());
    let cursor = props.cursor.min(props.rows.len().saturating_sub(1));
    for (index, row) in props.rows.iter().enumerate() {
        let label = format!(
            "{} {} · {}",
            if row.ticked { "☑" } else { "☐" },
            path::basename(&row.session.repo_root),
            session_name(&row.session)
        );
        let status = RowStatus {
            text: activity_text(props.kind, row.activity).to_owned(),
            // A row status takes the core theme's colour names.
            color: Some(if row.activity == InterruptedWhile::Idle {
                deckhand_core::display::THEME.muted
            } else {
                deckhand_core::display::THEME.warn
            }),
        };
        rows.body.extend(selectable_row(
            &SelectableRowProps {
                selected: index == cursor,
                text: &label,
                color: Some(if row.ticked { THEME.active } else { THEME.muted }),
                status: Some(&status),
                ..SelectableRowProps::default()
            },
            content_width,
        ));
    }
    rows.footer.push(Vec::new());
    let ticked = props.rows.iter().filter(|row| row.ticked).count();
    let enter = if ticked == 0 {
        "enter close".to_owned()
    } else {
        format!("enter {action} {ticked}")
    };
    let hint: Vec<HintPart> = vec![
        enter.as_str().into(),
        "space tick".into(),
        HintPart {
            text: "a all".to_owned(),
            short: None,
            drop: Some(1),
        },
        if props.kind == ResumeKind::Interrupted {
            "esc skip".into()
        } else {
            "esc cancel".into()
        },
    ];
    rows.footer
        .push(text(fit_hint(&hint, content_width, " • "), TextStyle::fg(THEME.muted)));
    rows
}

/// [`resume_dialog_rows`] drawn over `area` as `view` keeps it.
pub fn render_resume_dialog(
    props: &ResumeDialogProps<'_>,
    buf: &mut Buffer,
    area: Rect,
    view: DialogView,
) -> DialogScroll {
    let build = |width| resume_dialog_rows(props, width);
    draw_dialog(buf, area, &Border::round(THEME.border_active), &build, view)
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use serde_json::{Value, json};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::UnixStream;
    use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};

    use deckhand_client::live::{LiveClient, LiveClientHandlers};
    use deckhand_core::key::Key;

    use super::*;
    use crate::app::state::AppProps;
    use crate::app::test_support::app;

    fn record(id: &str, fields: Value) -> SessionRecord {
        let mut value = json!({
            "id": id, "title": id, "program": "claude", "command": "claude", "cwd": "/code/app",
            "repoRoot": "/code/app", "status": "exited", "createdAt": "2026-01-01T00:00:00.000Z",
            "updatedAt": "2026-01-01T00:00:00.000Z",
        });
        if let (Some(map), Some(extra)) = (value.as_object_mut(), fields.as_object()) {
            map.extend(extra.clone());
        }
        serde_json::from_value(value).expect("a session")
    }

    fn ids(rows: &[ResumeRow]) -> Vec<(&str, bool)> {
        rows.iter().map(|row| (row.session.id.as_str(), row.ticked)).collect()
    }

    fn key(setup: impl FnOnce(&mut Key)) -> Input {
        let mut key = Key::NONE;
        setup(&mut key);
        Input::key("", key)
    }

    #[test]
    fn interrupted_sessions_are_all_offered_ticked_and_grouped_by_repository() {
        let other = json!({"repoRoot": "/code/api", "cwd": "/code/api"});
        let rows = interrupted_rows(vec![
            record("a", json!({"interruptedWhile": "working"})),
            record(
                "b",
                json!({"interruptedWhile": "idle", "repoRoot": "/code/api", "cwd": "/code/api"}),
            ),
            // Interrupted before, or exited on its own: not offered.
            record("old", json!({"exitReason": "interrupted"})),
            record("c", json!({"interruptedWhile": "needs-input"})),
            // Started again since (the daemon clears the mark; a record in flight may still carry it).
            record("live", json!({"interruptedWhile": "idle", "status": "running"})),
            record("d", other.clone()),
        ]);
        assert_eq!(ids(&rows), [("a", true), ("c", true), ("b", true)]);
        assert_eq!(rows[1].activity, InterruptedWhile::NeedsInput);
    }

    #[test]
    fn outdated_sessions_leave_the_busy_ones_unticked() {
        let running = |id: &str, fields: Value| {
            let mut session = record(id, json!({"status": "running", "agentVersion": "2.1.287"}));
            if let Value::Object(fields) = fields {
                let mut value = serde_json::to_value(&session).unwrap();
                value.as_object_mut().unwrap().extend(fields);
                session = serde_json::from_value(value).unwrap();
            }
            session
        };
        let attention = |state: &str| json!({"state": state, "event": "e", "at": "2026-01-01T00:00:00.000Z"});
        let rows = outdated_rows(
            vec![
                running("idle", json!({"agentStatus": "idle"})),
                running("working", json!({"attention": attention("working")})),
                running("asking", json!({"attention": attention("needs-input")})),
                running("current", json!({"agentVersion": "2.1.290"})),
                running("codex", json!({"program": "codex"})),
                record("exited", json!({"agentVersion": "2.1.287"})),
            ],
            ProgramKey::Claude,
            Some("2.1.290"),
        );
        assert_eq!(ids(&rows), [("idle", true), ("working", false), ("asking", false)]);
    }

    #[test]
    fn keys_move_tick_and_leave_for_the_mode_it_opened_from() {
        let (mut app, _rx) = app(AppProps::default());
        app.set_mode(Mode::Agents);
        let rows = vec![
            ResumeRow {
                session: record("a", json!({})),
                activity: InterruptedWhile::Idle,
                ticked: true,
            },
            ResumeRow {
                session: record("b", json!({})),
                activity: InterruptedWhile::Working,
                ticked: false,
            },
        ];
        open(&mut app, ResumeKind::Outdated(ProgramKey::Claude), rows);
        assert_eq!(app.mode, Mode::ConfirmResume);
        handle_confirm_resume(&mut app, &Input::text("j"));
        handle_confirm_resume(&mut app, &Input::text(" "));
        assert_eq!(ids(&app.resume.rows), [("a", true), ("b", true)]);
        // a unticks every one when all are ticked, else ticks every one.
        handle_confirm_resume(&mut app, &Input::text("a"));
        assert_eq!(ids(&app.resume.rows), [("a", false), ("b", false)]);
        handle_confirm_resume(&mut app, &key(|key| key.down_arrow = true));
        assert_eq!(app.resume.cursor, 0);
        handle_confirm_resume(&mut app, &Input::text(" "));
        handle_confirm_resume(&mut app, &Input::text("a"));
        assert_eq!(ids(&app.resume.rows), [("a", true), ("b", true)]);
        handle_confirm_resume(&mut app, &key(|key| key.escape = true));
        assert_eq!(app.mode, Mode::Agents);
        assert!(app.resume.kind.is_none());
    }

    #[test]
    fn the_checklist_names_each_sessions_repository_and_what_it_was_doing() {
        let rows = vec![
            ResumeRow {
                session: record("tests", json!({"pathTitle": "Fix login / tests"})),
                activity: InterruptedWhile::Working,
                ticked: true,
            },
            ResumeRow {
                session: record("docs", json!({"repoRoot": "/code/api"})),
                activity: InterruptedWhile::Idle,
                ticked: false,
            },
        ];
        let props = ResumeDialogProps {
            kind: ResumeKind::Interrupted,
            rows: &rows,
            cursor: 1,
            installed: None,
        };
        let dialog = resume_dialog_rows(&props, 100);
        let lines: Vec<String> = [&dialog.header, &dialog.body, &dialog.footer]
            .into_iter()
            .flatten()
            .map(|row| {
                row.iter()
                    .map(|seg| seg.text.as_str())
                    .collect::<String>()
                    .trim_end()
                    .to_owned()
            })
            .collect();
        assert_eq!(lines[0], "Resume the sessions the daemon stopped?");
        let row = |name: &str| {
            lines
                .iter()
                .find(|line| line.contains(name))
                .cloned()
                .unwrap_or_default()
        };
        assert!(row("☑ app · Fix login / tests").ends_with("was working"), "{lines:#?}");
        assert!(row("☐ api · docs").ends_with("was idle"), "{lines:#?}");
        assert!(
            lines.last().unwrap().starts_with("enter resume 1 • space tick"),
            "{lines:#?}"
        );
        let outdated = ResumeDialogProps {
            kind: ResumeKind::Outdated(ProgramKey::Claude),
            installed: Some("2.1.290"),
            ..props
        };
        let dialog = resume_dialog_rows(&outdated, 100);
        let title: String = dialog.header[0].iter().map(|seg| seg.text.as_str()).collect();
        assert_eq!(title.trim_end(), "Restart Claude sessions on 2.1.290?");
    }

    struct Daemon {
        lines: BufReader<OwnedReadHalf>,
        writer: OwnedWriteHalf,
    }

    impl Daemon {
        async fn request(&mut self) -> Value {
            let mut line = String::new();
            self.lines.read_line(&mut line).await.expect("read");
            serde_json::from_str(&line).expect("json")
        }

        async fn respond(&mut self, request: &Value, data: Value) {
            let message = json!({"type": "response", "requestId": request["requestId"], "ok": true, "data": data});
            self.writer
                .write_all(format!("{message}\n").as_bytes())
                .await
                .expect("write");
        }
    }

    fn run(test: impl std::future::Future<Output = ()>) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        tokio::task::LocalSet::new().block_on(&runtime, test);
    }

    fn connected(app: &mut App) -> Daemon {
        let (client, server) = UnixStream::pair().expect("pair");
        let (read, writer) = server.into_split();
        app.connection.client = Some(Rc::new(LiveClient::new(client, LiveClientHandlers::default())));
        Daemon {
            lines: BufReader::new(read),
            writer,
        }
    }

    type Updates = tokio::sync::mpsc::UnboundedReceiver<crate::app::event_loop::Update<App>>;

    async fn apply(app: &mut App, rx: &mut Updates) {
        let update = rx.recv().await.expect("update");
        update(app);
    }

    #[test]
    fn resuming_interrupted_sessions_resumes_the_ticked_and_stops_offering_the_rest() {
        run(async {
            let (mut app, mut rx) = app(AppProps::default());
            let mut daemon = connected(&mut app);
            offer_interrupted(&mut app);
            let request = daemon.request().await;
            assert_eq!(request["type"], "list");
            let sessions = json!([
                record("a", json!({"interruptedWhile": "working"})),
                record("b", json!({"interruptedWhile": "idle"})),
                record("c", json!({"interruptedWhile": "idle"})),
            ]);
            daemon.respond(&request, sessions).await;
            apply(&mut app, &mut rx).await;
            assert_eq!(app.mode, Mode::ConfirmResume);
            assert_eq!(app.resume.kind, Some(ResumeKind::Interrupted));
            // Untick b, then resume a and c.
            handle_confirm_resume(&mut app, &Input::text("j"));
            handle_confirm_resume(&mut app, &Input::text(" "));
            handle_confirm_resume(&mut app, &key(|key| key.return_ = true));
            assert_eq!(app.mode, Mode::Browse);
            assert_eq!(app.messages.status_message.as_deref(), Some("Resuming 2 sessions…"));
            let dismiss = daemon.request().await;
            assert_eq!(dismiss["type"], "dismiss-interrupted");
            assert_eq!(dismiss["sessionIds"], json!(["b"]));
            let restart = daemon.request().await;
            assert_eq!(restart["type"], "restart-sessions");
            assert_eq!(restart["sessionIds"], json!(["a", "c"]));
            daemon.respond(&dismiss, json!({"ok": true})).await;
            apply(&mut app, &mut rx).await;
            // Meanwhile the status line counts the ones launched since.
            let poll = daemon.request().await;
            assert_eq!(poll["type"], "list");
            let launched = json!([
                record(
                    "a",
                    json!({"status": "running", "agentStartedAt": "2099-01-01T00:00:00.000Z"})
                ),
                record("c", json!({"agentStartedAt": "2026-01-01T00:00:00.000Z"})),
            ]);
            daemon.respond(&poll, launched).await;
            apply(&mut app, &mut rx).await;
            assert_eq!(app.messages.status_message.as_deref(), Some("Resuming 1 of 2…"));
            daemon
                .respond(
                    &restart,
                    json!([{"sessionId": "a"}, {"sessionId": "c", "error": "it did not stop"}]),
                )
                .await;
            apply(&mut app, &mut rx).await;
            assert_eq!(app.messages.status_message, None);
            assert_eq!(
                app.messages.error.as_deref(),
                Some("Resumed 1 of 2; not “c”: it did not stop")
            );
        });
    }

    #[test]
    fn nothing_to_resume_opens_nothing() {
        run(async {
            let (mut app, mut rx) = app(AppProps::default());
            let mut daemon = connected(&mut app);
            offer_interrupted(&mut app);
            let request = daemon.request().await;
            daemon.respond(&request, json!([record("a", json!({}))])).await;
            apply(&mut app, &mut rx).await;
            assert_eq!(app.mode, Mode::Browse);
            // Asked on the Agents screen, an empty list is said.
            app.set_mode(Mode::Agents);
            offer_outdated(&mut app, ProgramKey::Codex, true);
            let request = daemon.request().await;
            daemon.respond(&request, json!([])).await;
            apply(&mut app, &mut rx).await;
            assert_eq!(app.mode, Mode::Agents);
            assert_eq!(
                app.messages.status_message.as_deref(),
                Some("No running Codex session is on an older version")
            );
        });
    }
}
