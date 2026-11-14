//! Parsing `deckhand new|ls|task|note|kill|remove|guide` and their usage texts.
//!
//! Options take their value as the next argument or after `=` (`--title=x`), so a value may start with `-`. `--`
//! ends the options. Every command takes `--json` and `--help` (`-h`).

use deckhand_core::error::DhError;
use deckhand_protocol::data::ProgramKey;

/// The words that run one of these commands (the first argument).
pub const COMMAND_WORDS: &[&str] = &["new", "ls", "task", "note", "kill", "remove", "guide"];

/// `deckhand new --help`.
pub const NEW_USAGE: &str = "\
Usage: deckhand new [--prompt <text>] [--draft] [--agent claude|codex|pi] [--title <title>] [--task <id>]
                    [--worktree new|same|none|<path>] [--base <branch>] [--flat] [--top-level] [--fork] [--json]

Starts a session. Run inside a Deckhand session, it is a sub-session of that session in a new worktree; run
elsewhere, a top-level session of the current repository.

  --prompt <text>     The agent's first message, sent when it starts (from a shell, a long one can come from
                      a quoted heredoc: --prompt \"$(cat <<'EOF' ... EOF)\")
  --draft             Type the prompt into the agent's input without sending it
  --agent <agent>     claude, codex or pi (default: this session's agent, else the repository's default agent)
  --title <title>     The session's name (default: the task's title, else the prompt's first line)
  --task <id>         Start on a task from `deckhand task list` (top-level unless in a new worktree; the
                      prompt, if any, replaces the task's text; without one the task's text is typed as a draft)
  --worktree <where>  new: a new worktree (the default); same: this session's worktree; none: no worktree,
                      in the repository's checkout; <path>: an existing worktree
  --base <branch>     The branch a new worktree starts from (default: a sub-session's is stacked on this
                      session's worktree branch, starting from it and merging into it; else the repository's
                      setting)
  --flat              A sub-session's new worktree is not stacked: it starts from the base above and merges
                      into the repository's checkout
  --top-level         A top-level session instead of a sub-session of this one
  --fork              Copy this session's conversation (same agent; Codex forks stay in this worktree)
  --json              Print the new session's record as JSON";

/// `deckhand ls --help`.
pub const LS_USAGE: &str = "\
Usage: deckhand ls [--json]

Lists the sessions of the checkout it runs in (archived ones left out): ID, state, agent, name (a sub-session under
its parent's), worktree branch and task. * marks the session the command runs in. --json prints the records.";

/// `deckhand task --help`.
pub const TASK_USAGE: &str = "\
Usage: deckhand task list [--json]
       deckhand task add <title> [--details <text>] [--json]
       deckhand task done <id> [--json]
       deckhand task reopen <id> [--json]
       deckhand task move <id> --to backlog|<session id> [--json]

The current repository's tasks (the Tasks board, b).
  list    Open tasks (in progress, then the backlog) and those done in the last 7 days
  add     Adds a task to the backlog
  done    Ticks a task; reopen unticks it
  move    Moves a task to the backlog, or makes it the task of a session's work (its worktree, or the
          session in the main checkout); when that work has a task already, it becomes a step in its note
IDs may be shortened to any unique prefix.";

/// `deckhand note --help`.
pub const NOTE_USAGE: &str = "\
Usage: deckhand note add <item> [--json]

Adds `- [ ] <item>` to the end of this session's note: its worktree's note, or in the main checkout the
session's own. Only inside a Deckhand session.";

/// `deckhand kill --help`.
pub const KILL_USAGE: &str = "\
Usage: deckhand kill <session id> [--json]

Ends a running session. Inside a Deckhand session, only a sub-session this session started. Its worktree and
branch are kept.";

/// `deckhand remove --help`.
pub const REMOVE_USAGE: &str = "\
Usage: deckhand remove <session id> [--json]

Removes an ended session from the list. Inside a Deckhand session, only a sub-session this session started.
Its worktree and branch are kept.";

/// `deckhand guide --help`.
pub const GUIDE_USAGE: &str = "\
Usage: deckhand guide

Prints the guide to these commands for agents running in a Deckhand session.";

/// One command and whether `--json` was given.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Invocation {
    pub action: Action,
    pub json: bool,
}

/// What to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// `<command> --help`: print this usage.
    Help(&'static str),
    Guide,
    New(NewArgs),
    Ls,
    TaskList,
    TaskAdd {
        title: String,
        details: Option<String>,
    },
    TaskDone(String),
    TaskReopen(String),
    TaskMove {
        id: String,
        to: MoveTo,
    },
    NoteAdd(String),
    Kill(String),
    Remove(String),
}

/// `task move --to`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MoveTo {
    Backlog,
    /// A session ID (or a prefix of one).
    Session(String),
}

/// `new --worktree`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorktreeChoice {
    New,
    Same,
    None,
    Path(String),
}

/// `deckhand new`'s options.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NewArgs {
    pub prompt: Option<String>,
    pub draft: bool,
    pub agent: Option<ProgramKey>,
    pub title: Option<String>,
    pub task: Option<String>,
    pub worktree: Option<WorktreeChoice>,
    pub base: Option<String>,
    pub flat: bool,
    pub top_level: bool,
    pub fork: bool,
}

/// Whether a command needs the daemon (and the state directory): all but `guide` and `--help`.
pub fn needs_daemon(args: &[String]) -> bool {
    !matches!(
        parse(args).map(|invocation| invocation.action),
        Ok(Action::Guide | Action::Help(_))
    )
}

/// A usage error: `problem`, then the usage lines (up to the first blank line).
fn usage_error(problem: &str, usage: &str) -> DhError {
    let lines = usage.split("\n\n").next().unwrap_or(usage);
    DhError::msg(format!("{problem}\n{lines}"))
}

/// Whether an option takes a value.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Switch,
    Value,
}

/// The arguments of one command, scanned.
#[derive(Default)]
struct Scanned {
    values: Vec<(&'static str, String)>,
    switches: Vec<&'static str>,
    positionals: Vec<String>,
    json: bool,
    help: bool,
}

impl Scanned {
    fn value(&self, name: &str) -> Option<String> {
        self.values
            .iter()
            .find(|(option, _)| *option == name)
            .map(|(_, value)| value.clone())
    }

    fn switch(&self, name: &str) -> bool {
        self.switches.contains(&name)
    }
}

/// Splits `args` into the options of `options`, `--json`, `--help` and positionals; an unknown option, a missing
/// value or an option given twice is refused with `usage`.
fn scan(args: &[String], options: &[(&'static str, Kind)], usage: &str) -> Result<Scanned, DhError> {
    let mut scanned = Scanned::default();
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        if arg == "--" {
            scanned.positionals.extend(rest.by_ref().cloned());
            break;
        }
        match arg.as_str() {
            "--json" => {
                scanned.json = true;
                continue;
            }
            "--help" | "-h" => {
                scanned.help = true;
                continue;
            }
            _ => {}
        }
        if !arg.starts_with('-') || arg == "-" {
            scanned.positionals.push(arg.clone());
            continue;
        }
        let (name, inline) = match arg.split_once('=') {
            Some((name, value)) if name.starts_with("--") => (name, Some(value.to_owned())),
            _ => (arg.as_str(), None),
        };
        let Some((option, kind)) = options.iter().find(|(option, _)| *option == name) else {
            return Err(usage_error(&format!("Unknown option: {name}"), usage));
        };
        let seen = scanned.switches.contains(option) || scanned.values.iter().any(|(seen, _)| seen == option);
        if seen {
            return Err(usage_error(&format!("{option} is given twice"), usage));
        }
        match (kind, inline) {
            (Kind::Switch, None) => scanned.switches.push(option),
            (Kind::Switch, Some(_)) => {
                return Err(usage_error(&format!("{option} takes no value"), usage));
            }
            (Kind::Value, Some(value)) => scanned.values.push((option, value)),
            (Kind::Value, None) => match rest.next() {
                Some(value) => scanned.values.push((option, value.clone())),
                None => return Err(usage_error(&format!("{option} needs a value"), usage)),
            },
        }
    }
    Ok(scanned)
}

/// The one positional a command takes (`what` names it in the error).
fn one(scanned: &Scanned, what: &str, command: &str, usage: &str) -> Result<String, DhError> {
    match scanned.positionals.as_slice() {
        [only] if !only.trim().is_empty() => Ok(only.clone()),
        [] | [_] => Err(usage_error(&format!("deckhand {command} needs {what}"), usage)),
        [_, extra, ..] => Err(usage_error(&format!("Unexpected argument: {extra}"), usage)),
    }
}

/// Positionals joined by spaces (`note add Write the tests`), which must not be blank.
fn words(scanned: &Scanned, what: &str, command: &str, usage: &str) -> Result<String, DhError> {
    let text = scanned.positionals.join(" ");
    if text.trim().is_empty() {
        return Err(usage_error(&format!("deckhand {command} needs {what}"), usage));
    }
    Ok(text)
}

/// No positionals.
fn none(scanned: &Scanned, usage: &str) -> Result<(), DhError> {
    match scanned.positionals.first() {
        Some(extra) => Err(usage_error(&format!("Unexpected argument: {extra}"), usage)),
        None => Ok(()),
    }
}

/// Parses one of [`COMMAND_WORDS`] and its arguments (`args` starts with the command word).
pub fn parse(args: &[String]) -> Result<Invocation, DhError> {
    let word = args.first().map(String::as_str).unwrap_or_default();
    let rest = args.get(1..).unwrap_or_default();
    match word {
        "new" => parse_new(rest),
        "ls" => simple(rest, LS_USAGE, Action::Ls),
        "guide" => simple(rest, GUIDE_USAGE, Action::Guide),
        "task" => parse_task(rest),
        "note" => parse_note(rest),
        "kill" | "remove" => {
            let usage = if word == "kill" { KILL_USAGE } else { REMOVE_USAGE };
            let scanned = scan(rest, &[], usage)?;
            if scanned.help {
                return Ok(help(usage));
            }
            let id = one(&scanned, "a session ID", word, usage)?;
            let action = if word == "kill" {
                Action::Kill(id)
            } else {
                Action::Remove(id)
            };
            Ok(Invocation {
                action,
                json: scanned.json,
            })
        }
        other => Err(DhError::msg(format!("Unknown command: {other}"))),
    }
}

fn help(usage: &'static str) -> Invocation {
    Invocation {
        action: Action::Help(usage),
        json: false,
    }
}

/// A command without options or positionals.
fn simple(rest: &[String], usage: &'static str, action: Action) -> Result<Invocation, DhError> {
    let scanned = scan(rest, &[], usage)?;
    if scanned.help {
        return Ok(help(usage));
    }
    none(&scanned, usage)?;
    Ok(Invocation {
        action,
        json: scanned.json,
    })
}

fn parse_agent(text: &str) -> Result<ProgramKey, DhError> {
    ProgramKey::ALL
        .iter()
        .copied()
        .find(|program| program.as_str() == text)
        .ok_or_else(|| usage_error(&format!("Unknown agent: {text} (claude, codex or pi)"), NEW_USAGE))
}

fn parse_worktree(text: &str) -> Result<WorktreeChoice, DhError> {
    Ok(match text {
        "new" => WorktreeChoice::New,
        "same" => WorktreeChoice::Same,
        "none" => WorktreeChoice::None,
        "" => return Err(usage_error("--worktree needs a value", NEW_USAGE)),
        path => WorktreeChoice::Path(path.to_owned()),
    })
}

fn parse_new(rest: &[String]) -> Result<Invocation, DhError> {
    let options = [
        ("--prompt", Kind::Value),
        ("--draft", Kind::Switch),
        ("--agent", Kind::Value),
        ("--title", Kind::Value),
        ("--task", Kind::Value),
        ("--worktree", Kind::Value),
        ("--base", Kind::Value),
        ("--flat", Kind::Switch),
        ("--top-level", Kind::Switch),
        ("--fork", Kind::Switch),
    ];
    let scanned = scan(rest, &options, NEW_USAGE)?;
    if scanned.help {
        return Ok(help(NEW_USAGE));
    }
    none(&scanned, NEW_USAGE)?;
    let new = NewArgs {
        prompt: scanned.value("--prompt").filter(|prompt| !prompt.trim().is_empty()),
        draft: scanned.switch("--draft"),
        agent: scanned.value("--agent").as_deref().map(parse_agent).transpose()?,
        title: scanned.value("--title"),
        task: scanned.value("--task"),
        worktree: scanned.value("--worktree").as_deref().map(parse_worktree).transpose()?,
        base: scanned.value("--base").filter(|base| !base.is_empty()),
        flat: scanned.switch("--flat"),
        top_level: scanned.switch("--top-level"),
        fork: scanned.switch("--fork"),
    };
    if new.draft && new.prompt.is_none() {
        return Err(usage_error("--draft needs --prompt", NEW_USAGE));
    }
    if new.task.is_some() && new.fork {
        return Err(usage_error("--task and --fork cannot be combined", NEW_USAGE));
    }
    if new.base.is_some() && !matches!(new.worktree, None | Some(WorktreeChoice::New)) {
        return Err(usage_error("--base applies only to a new worktree", NEW_USAGE));
    }
    if new.flat && !matches!(new.worktree, None | Some(WorktreeChoice::New)) {
        return Err(usage_error("--flat applies only to a new worktree", NEW_USAGE));
    }
    Ok(Invocation {
        action: Action::New(new),
        json: scanned.json,
    })
}

fn parse_task(rest: &[String]) -> Result<Invocation, DhError> {
    let usage = TASK_USAGE;
    let sub = rest.first().map(String::as_str);
    let args = rest.get(1..).unwrap_or_default();
    let options: &[(&'static str, Kind)] = match sub {
        Some("add") => &[("--details", Kind::Value)],
        Some("move") => &[("--to", Kind::Value)],
        _ => &[],
    };
    let scanned = scan(args, options, usage)?;
    if scanned.help || matches!(sub, None | Some("--help" | "-h")) {
        return Ok(help(usage));
    }
    let action = match sub {
        Some("list") => {
            none(&scanned, usage)?;
            Action::TaskList
        }
        Some("add") => Action::TaskAdd {
            title: words(&scanned, "a title", "task add", usage)?,
            details: scanned.value("--details").filter(|details| !details.trim().is_empty()),
        },
        Some("done") => Action::TaskDone(one(&scanned, "a task ID", "task done", usage)?),
        Some("reopen") => Action::TaskReopen(one(&scanned, "a task ID", "task reopen", usage)?),
        Some("move") => {
            let id = one(&scanned, "a task ID", "task move", usage)?;
            let to = match scanned.value("--to").as_deref() {
                None | Some("") => return Err(usage_error("deckhand task move needs --to", usage)),
                Some("backlog") => MoveTo::Backlog,
                Some(session) => MoveTo::Session(session.to_owned()),
            };
            Action::TaskMove { id, to }
        }
        Some(other) => return Err(usage_error(&format!("Unknown task command: {other}"), usage)),
        None => return Ok(help(usage)),
    };
    Ok(Invocation {
        action,
        json: scanned.json,
    })
}

fn parse_note(rest: &[String]) -> Result<Invocation, DhError> {
    let usage = NOTE_USAGE;
    let sub = rest.first().map(String::as_str);
    let scanned = scan(rest.get(1..).unwrap_or_default(), &[], usage)?;
    if scanned.help || matches!(sub, None | Some("--help" | "-h")) {
        return Ok(help(usage));
    }
    match sub {
        Some("add") => Ok(Invocation {
            action: Action::NoteAdd(words(&scanned, "an item", "note add", usage)?),
            json: scanned.json,
        }),
        Some(other) => Err(usage_error(&format!("Unknown note command: {other}"), usage)),
        None => Ok(help(usage)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|arg| (*arg).to_owned()).collect()
    }

    fn parsed(list: &[&str]) -> Invocation {
        parse(&args(list)).unwrap_or_else(|error| panic!("{list:?}: {error}"))
    }

    fn refused(list: &[&str]) -> String {
        match parse(&args(list)) {
            Ok(invocation) => panic!("{list:?} parsed: {invocation:?}"),
            Err(error) => error.to_string(),
        }
    }

    fn new(list: &[&str]) -> NewArgs {
        match parsed(&[&["new"], list].concat()).action {
            Action::New(new) => new,
            other => panic!("not new: {other:?}"),
        }
    }

    #[test]
    fn new_without_options_is_a_default_sub_session() {
        assert_eq!(new(&[]), NewArgs::default());
        assert!(!parsed(&["new"]).json);
    }

    #[test]
    fn new_reads_every_option_with_its_value_next_or_after_an_equals_sign() {
        let all = new(&[
            "--prompt",
            "Fix the login bug.\nRun the tests.",
            "--draft",
            "--agent=codex",
            "--title",
            "Login",
            "--task",
            "ab12",
            "--worktree",
            "new",
            "--base=main",
            "--flat",
            "--top-level",
        ]);
        assert_eq!(
            all,
            NewArgs {
                prompt: Some("Fix the login bug.\nRun the tests.".into()),
                draft: true,
                agent: Some(ProgramKey::Codex),
                title: Some("Login".into()),
                task: Some("ab12".into()),
                worktree: Some(WorktreeChoice::New),
                base: Some("main".into()),
                flat: true,
                top_level: true,
                fork: false,
            }
        );
        assert!(new(&["--fork"]).fork);
    }

    #[test]
    fn a_prompt_may_start_with_a_dash_or_hold_internal_flags() {
        assert_eq!(new(&["--prompt", "--daemon"]).prompt.as_deref(), Some("--daemon"));
        assert_eq!(new(&["--prompt=-x"]).prompt.as_deref(), Some("-x"));
        // A blank prompt is no prompt.
        assert_eq!(new(&["--prompt", "  "]).prompt, None);
    }

    #[test]
    fn worktree_takes_a_mode_or_a_path() {
        assert_eq!(new(&["--worktree", "same"]).worktree, Some(WorktreeChoice::Same));
        assert_eq!(new(&["--worktree", "none"]).worktree, Some(WorktreeChoice::None));
        assert_eq!(
            new(&["--worktree", "../other"]).worktree,
            Some(WorktreeChoice::Path("../other".into()))
        );
    }

    #[test]
    fn new_refuses_what_cannot_be_combined_or_is_unknown() {
        assert_eq!(
            refused(&["new", "--agent", "gpt"]).lines().next(),
            Some("Unknown agent: gpt (claude, codex or pi)")
        );
        assert!(refused(&["new", "--draft"]).starts_with("--draft needs --prompt\nUsage: deckhand new"));
        assert!(refused(&["new", "--task", "a", "--fork"]).starts_with("--task and --fork cannot be combined"));
        assert!(refused(&["new", "--base", "main", "--worktree", "same"]).starts_with("--base applies only"));
        assert!(refused(&["new", "--flat", "--worktree", "none"]).starts_with("--flat applies only"));
        assert!(refused(&["new", "--prompt"]).starts_with("--prompt needs a value"));
        assert!(refused(&["new", "--title", "a", "--title", "b"]).starts_with("--title is given twice"));
        assert!(refused(&["new", "--fork=yes"]).starts_with("--fork takes no value"));
        assert!(refused(&["new", "--yes"]).starts_with("Unknown option: --yes"));
        assert!(refused(&["new", "stray"]).starts_with("Unexpected argument: stray"));
    }

    #[test]
    fn usage_errors_end_with_the_usage_lines_only() {
        let error = refused(&["kill"]);
        assert_eq!(
            error,
            "deckhand kill needs a session ID\nUsage: deckhand kill <session id> [--json]"
        );
    }

    #[test]
    fn every_command_prints_its_usage_for_help() {
        for (list, usage) in [
            (&["new", "--help"][..], NEW_USAGE),
            (&["new", "--prompt", "x", "-h"], NEW_USAGE),
            (&["ls", "--help"], LS_USAGE),
            (&["task"], TASK_USAGE),
            (&["task", "--help"], TASK_USAGE),
            (&["task", "add", "--help"], TASK_USAGE),
            (&["note"], NOTE_USAGE),
            (&["note", "add", "-h"], NOTE_USAGE),
            (&["kill", "--help"], KILL_USAGE),
            (&["remove", "--help"], REMOVE_USAGE),
            (&["guide", "--help"], GUIDE_USAGE),
        ] {
            assert_eq!(parsed(list).action, Action::Help(usage), "{list:?}");
        }
    }

    #[test]
    fn task_commands_take_their_ids_titles_and_targets() {
        assert_eq!(
            parsed(&["task", "list", "--json"]),
            Invocation {
                action: Action::TaskList,
                json: true
            }
        );
        assert_eq!(
            parsed(&["task", "add", "Write", "the", "docs", "--details", "For the CLI"]).action,
            Action::TaskAdd {
                title: "Write the docs".into(),
                details: Some("For the CLI".into())
            }
        );
        assert_eq!(
            parsed(&["task", "add", "--", "--odd title"]).action,
            Action::TaskAdd {
                title: "--odd title".into(),
                details: None
            }
        );
        assert_eq!(
            parsed(&["task", "done", "ab12"]).action,
            Action::TaskDone("ab12".into())
        );
        assert_eq!(
            parsed(&["task", "reopen", "ab12"]).action,
            Action::TaskReopen("ab12".into())
        );
        assert_eq!(
            parsed(&["task", "move", "ab12", "--to", "backlog"]).action,
            Action::TaskMove {
                id: "ab12".into(),
                to: MoveTo::Backlog
            }
        );
        assert_eq!(
            parsed(&["task", "move", "ab12", "--to=1f2e"]).action,
            Action::TaskMove {
                id: "ab12".into(),
                to: MoveTo::Session("1f2e".into())
            }
        );
        assert!(refused(&["task", "move", "ab12"]).starts_with("deckhand task move needs --to"));
        assert!(refused(&["task", "add"]).starts_with("deckhand task add needs a title"));
        assert!(refused(&["task", "done", "a", "b"]).starts_with("Unexpected argument: b"));
        assert!(refused(&["task", "list", "--to", "x"]).starts_with("Unknown option: --to"));
        assert!(refused(&["task", "tick", "a"]).starts_with("Unknown task command: tick"));
    }

    #[test]
    fn note_add_joins_its_words_and_other_commands_take_one_id() {
        assert_eq!(
            parsed(&["note", "add", "Check", "the", "migration"]).action,
            Action::NoteAdd("Check the migration".into())
        );
        assert!(refused(&["note", "add", " "]).starts_with("deckhand note add needs an item"));
        assert!(refused(&["note", "list"]).starts_with("Unknown note command: list"));
        assert_eq!(
            parsed(&["kill", "1f2e", "--json"]),
            Invocation {
                action: Action::Kill("1f2e".into()),
                json: true
            }
        );
        assert_eq!(parsed(&["remove", "1f2e"]).action, Action::Remove("1f2e".into()));
        assert_eq!(parsed(&["ls"]).action, Action::Ls);
        assert!(refused(&["ls", "all"]).starts_with("Unexpected argument: all"));
        assert_eq!(parsed(&["guide"]).action, Action::Guide);
    }

    #[test]
    fn the_guide_names_every_command_and_option_of_new() {
        let guide = deckhand_core::agent_guide::GUIDE;
        for word in COMMAND_WORDS.iter().filter(|word| **word != "guide") {
            assert!(guide.contains(&format!("deckhand {word}")), "{word}");
        }
        for listed in [
            "task list",
            "task add",
            "task done|reopen",
            "task move",
            "note add",
            "--details",
            "--to",
        ] {
            assert!(guide.contains(listed), "{listed}");
        }
        let options = NEW_USAGE
            .split(|c: char| c.is_whitespace() || c == '[' || c == ']')
            .filter(|word| word.starts_with("--") && *word != "--json");
        for option in options {
            assert!(guide.contains(option), "{option}");
        }
    }

    #[test]
    fn only_guide_and_help_run_without_the_daemon() {
        assert!(!needs_daemon(&args(&["guide"])));
        assert!(!needs_daemon(&args(&["new", "--help"])));
        assert!(!needs_daemon(&args(&["task"])));
        assert!(needs_daemon(&args(&["ls"])));
        assert!(needs_daemon(&args(&["new"])));
        // A usage error is reported after the state directory check, as any command.
        assert!(needs_daemon(&args(&["kill"])));
    }
}
