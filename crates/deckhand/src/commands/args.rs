//! Parsing `deckhand new|ls|task|step|note|send|reply|kill|remove|guide|plan|do` and their usage texts.
//!
//! Options take their value as the next argument or after `=` (`--title=x`), so a value may start with `-`. `--`
//! ends the options. Every command takes `--json` and `--help` (`-h`). A long text (a prompt, details, a note, a
//! message, an answer, an idea) may be `-`: read from stdin (`commands::read_stdin`), never the literal `-`.

use deckhand_core::error::DhError;
use deckhand_protocol::data::ProgramKey;

/// The words that run one of these commands (the first argument).
pub const COMMAND_WORDS: &[&str] = &[
    "new", "ls", "task", "step", "note", "send", "reply", "kill", "remove", "guide", "plan", "do",
];

/// The commands every agent may run (its guide's, and Codex's `deckhand` tool's): not the orchestrator's `plan`
/// (Claude only) or your `do`.
pub const AGENT_WORDS: &[&str] = &[
    "new", "ls", "task", "step", "note", "send", "reply", "kill", "remove", "guide",
];

/// `deckhand new --help`.
pub const NEW_USAGE: &str = "\
Usage: deckhand new [--prompt <text>|-] [--draft] [--agent claude|codex|pi] [--title <title>] [--task <id>]
                    [--parent <id>] [--worktree new|same|none|<path>] [--base <branch>] [--flat] [--top-level]
                    [--fork] [--json]

Starts a session. Run inside a Deckhand session, it is a sub-session of that session in a new worktree; run
elsewhere, a top-level session of the current repository.

  --prompt <text>|-   The agent's first message, sent when it starts. - reads it from stdin, for a long one
                      (a quoted heredoc: deckhand new --prompt - <<'EOF', the brief, then EOF alone on a line)
  --draft             Type the prompt into the agent's input without sending it
  --agent <agent>     claude, codex or pi (default: this session's agent, else the repository's default agent)
  --title <title>     The session's name: 2-4 words naming the work, not a sentence (default: the task's title,
                      else the prompt's first sentence)
  --task <id>         Start on a line from `deckhand task list` that nobody is on: the session is on it (the
                      prompt, if any, replaces its text; without one its title, brief and note are typed as a
                      draft)
  --parent <id>       A sub-session of that session instead: inside a session, one under this session (at any
                      depth); run by hand, any session of the repository
  --worktree <where>  new: a new worktree (the default); same: the parent's (else this session's) worktree;
                      none: no worktree, in the repository's checkout; <path>: an existing worktree
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
Usage: deckhand ls [--all] [--json]

Inside a Deckhand session: this session's tree, the sessions you can message with deckhand send. That is its
top-level session and every session under it, in whichever checkout each one is; indented under their parents,
* marking this session, ◎ an orchestrator. Run by hand, or with --all: the sessions of the checkout it runs in,
with each one's parent. Archived sessions are left out. Columns: ID, state, agent, name, worktree branch, task.

  --all   The sessions of this checkout, not this session's tree
  --json  Print the records, in the same order";

/// `deckhand task --help`.
pub const TASK_USAGE: &str = "\
Usage: deckhand task list [--json]
       deckhand task add <title> [--details <text>|-] [--json]
       deckhand task done <id> [--json]
       deckhand task reopen <id> [--json]
       deckhand task move <id> --to backlog|<session id>|<line id> [--json]

The current repository's list (the Tasks board, b): one tree of lines; a session is on at most one line, and the
lines under it are its steps.
  list    The tree: open lines and those ticked in the last 7 days, who is on each, ticked/total of the lines
          under it, a brief (✎)
  add     Adds a line to the backlog (--details: its note; - reads it from stdin, as a quoted heredoc:
          deckhand task add <title> --details - <<'EOF' ... EOF)
  done    Ticks a line; reopen unticks it. A line whose lines are all ticked ticks itself
  move    Moves a line, with its lines: to the backlog (nobody on it); onto a session's line, as a step of it
          (onto a session without a line, it becomes its line); under another line
IDs may be shortened to any unique prefix.";

/// `deckhand step --help`.
pub const STEP_USAGE: &str = "\
Usage: deckhand step add <text> [--json]
       deckhand step done <id> [--json]
       deckhand step reopen <id> [--json]
       deckhand step list [--json]

This session's steps: the lines under its line in the list (the line is made, named after the session, the first
time it needs one). Only inside a Deckhand session.
  add     Adds a step at the end
  done    Ticks a step (any line under this session's line); reopen unticks it
  list    This session's line and its steps
IDs may be shortened to any unique prefix.";

/// `deckhand note --help`.
pub const NOTE_USAGE: &str = "\
Usage: deckhand note add <text>|- [--json]

Appends <text> as a paragraph to the note of this session's line (findings, decisions, context for whoever works
on it next; the line is made the first time it needs one). - reads the text from stdin (a quoted heredoc: deckhand
note add - <<'EOF' ... EOF). Steps go in with deckhand step add. Only inside a Deckhand session.";

/// `deckhand send --help`.
pub const SEND_USAGE: &str = "\
Usage: deckhand send <session id> <message>|- [--wait [--timeout <duration>]] [--json]
       deckhand send --cancel <message id> [--json]

Sends a message to another session: it is queued, then typed into that session's agent and submitted once the
agent is idle at its input (not while it starts, works on a turn or waits on a question or permission, nor while
you are typing into it or have typed text there you have not sent). Messages to a session arrive one at a time, in
the order sent, under a header saying who sent them. Inside a Deckhand session, only to a session in its tree
(everything under the same top-level session); run by hand, to any session. At most 20 KB: put more in your line's
note (deckhand note add) and point at it. Prints the message's ID; the receiver answers with deckhand reply.

  -                  Read the message from stdin (a quoted heredoc: deckhand send <id> - <<'EOF' ... EOF)
  --wait             Wait for the first reply and print it (run it with a long timeout: Bash timeout 600000).
                     It returns early, with the exit status in brackets, when the session waits on a question
                     or permission (3), ends its turn without replying (4; agents that report turns: Claude,
                     Codex with hooks), exits (5), or did not start a turn after the message was typed (6);
                     after --timeout too (2). A later reply then comes to this session as a message.
  --timeout <time>   How long --wait waits: 90s, 5m, 1h (default 9m)
  --cancel <id>      Cancels a message this session sent that is still queued (by hand: any)
IDs may be shortened to any unique prefix.";

/// `deckhand reply --help`.
pub const REPLY_USAGE: &str = "\
Usage: deckhand reply <message id> <answer>|- [--json]

Answers a message sent to this session (its header has the ID). The answer goes to the sender: to its waiting
deckhand send --wait, else it is queued and typed into the sender as a message (a message you sent by hand: back to
you). You may reply more than once. Only inside the Deckhand session the message was sent to. - reads the answer
from stdin. At most 20 KB.";

/// `deckhand kill --help`.
pub const KILL_USAGE: &str = "\
Usage: deckhand kill <session id> [--json]

Ends a running session. Inside a Deckhand session, only a session under it (a sub-session, at any depth). Its
worktree and branch are kept.";

/// `deckhand remove --help`.
pub const REMOVE_USAGE: &str = "\
Usage: deckhand remove <session id> [--json]

Removes an ended session from the list. Inside a Deckhand session, only a session under it (a sub-session, at any
depth). Its worktree and branch are kept.";

/// `deckhand plan --help`.
pub const PLAN_USAGE: &str = "\
Usage: deckhand plan check [<file>|-] [--json]
       deckhand plan apply [<file>|-] [--json]

An orchestrator plan: sessions (a tree, each with its brief), parked work and the idea's brief, as JSON from a file
or stdin (-, the default). Every problem is reported at once, and nothing is created while there is one.
  check   Validates the plan and prints what it would create
  apply   Validates it and returns at once: it waits for you in Deckhand (enter creates it, esc puts it aside,
          x x cancels it), and the orchestrator that applied it is told your answer in a message. Only in an
          orchestrator session, or by hand.

{\"sessions\": [{\"key\": \"auth\", \"title\": \"Auth rework\", \"prompt\": \"<brief>\", \"agent\": \"claude\",
               \"parent\": null, \"worktree\": \"new\", \"base\": null, \"flat\": false, \"task\": null},
              {\"key\": \"tokens\", \"title\": \"Token storage\", \"prompt\": \"<brief>\", \"parent\": \"auth\"}],
 \"tasks\": [{\"title\": \"Remove legacy endpoint\", \"details\": \"After Auth rework merges: ...\"}],
 \"brief\": \"<what was agreed>\"}

  key       Unique in the plan; other sessions name it as their parent
  title     At most 64 characters; prompt: the brief, sent as the session's first message
  agent     claude, codex or pi (default: the repository's default agent, else claude)
  parent    Another session's key, an existing session's ID (or a unique prefix), or null: a sub-session of the
            orchestrator applying it (by hand: a top-level session)
  worktree  new (the default), same (its parent's worktree) or none (the repository's checkout)
  base      Where a new worktree's branch starts; flat: true keeps a sub-session's new worktree from stacking on
            its parent's branch
  task      An open line's ID that nobody is on: the session takes it on
  brief     What was agreed (the problem, the approach, decisions, what was ruled out): the idea's line's brief.
            Only in an orchestrator's plan
Sessions are created parents first, each with its brief sent (and kept as its line's brief); then the tasks become
lines under the idea's line, nobody on them (by hand: in the backlog), their details their note.";

/// `deckhand do --help`.
pub const DO_USAGE: &str = "\
Usage: deckhand do [--title <name>] <idea>|- [--json]

Starts a new orchestrator for an idea, which splits it into sessions for you to confirm: the idea is its first
message, and a new top-level line of the orchestrator's name is the idea's, with the orchestrator on it. Every call
starts a new one; several can run at once. - reads the idea from stdin. Run it from a terminal in the repository,
not inside a session.

  --title <name>  The orchestrator's name and its line's: short, 2-4 words (at most 64 characters; longer is cut
                  at a word). Default: the idea's first line
  --json          Print the new orchestrator's record and its line's ID as JSON";

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
    /// `ls`; `--all`: the checkout's sessions even inside a session.
    Ls {
        all: bool,
    },
    TaskList,
    TaskAdd {
        title: String,
        details: Option<TextSource>,
    },
    TaskDone(String),
    TaskReopen(String),
    TaskMove {
        id: String,
        to: MoveTo,
    },
    StepAdd(String),
    StepDone(String),
    StepReopen(String),
    StepList,
    NoteAdd(TextSource),
    /// `send <session> <message>`.
    Send(SendArgs),
    /// `send --cancel <message id>`.
    CancelMessage(String),
    /// `reply <message id> <answer>`.
    Reply {
        id: String,
        text: TextSource,
    },
    Kill(String),
    Remove(String),
    /// `plan check`.
    PlanCheck(PlanSource),
    /// `plan apply`.
    PlanApply(PlanSource),
    /// `do [--title <name>] <idea>|-`.
    Do {
        idea: TextSource,
        title: Option<String>,
    },
}

/// A long text (a prompt, details, a note, a message, an answer, an idea): given, or `-` (stdin).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TextSource {
    Given(String),
    Stdin,
}

impl TextSource {
    /// The text, once given (`commands::run` reads stdin into a `-` before acting).
    pub fn given(&self) -> Option<&str> {
        match self {
            TextSource::Given(text) => Some(text),
            TextSource::Stdin => None,
        }
    }
}

/// The one text of an action that is `-`, to be read from stdin.
pub struct StdinText<'a> {
    pub slot: &'a mut TextSource,
    /// What it is, for an error (`the prompt`).
    pub what: &'static str,
    /// The command as it reads it (`deckhand new --prompt -`), for an error.
    pub form: &'static str,
}

impl Action {
    /// The text this action reads from stdin, if it was given as `-` (a command has at most one).
    pub fn stdin_text(&mut self) -> Option<StdinText<'_>> {
        let (slot, what, form) = match self {
            Action::New(NewArgs { prompt: Some(slot), .. }) => (slot, "the prompt", "deckhand new --prompt -"),
            Action::TaskAdd {
                details: Some(slot), ..
            } => (slot, "the details", "deckhand task add <title> --details -"),
            Action::NoteAdd(slot) => (slot, "the note", "deckhand note add -"),
            Action::Send(SendArgs { text: slot, .. }) => (slot, "the message", "deckhand send <id> -"),
            Action::Reply { text: slot, .. } => (slot, "the answer", "deckhand reply <message id> -"),
            Action::Do { idea: slot, .. } => (slot, "the idea", "deckhand do -"),
            _ => return None,
        };
        if *slot == TextSource::Stdin {
            Some(StdinText { slot, what, form })
        } else {
            None
        }
    }
}

/// `deckhand send`'s arguments.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SendArgs {
    /// A session ID (or a prefix of one).
    pub to: String,
    pub text: TextSource,
    /// `--wait`, with its timeout in milliseconds (default [`DEFAULT_WAIT_MS`]).
    pub wait: Option<u64>,
}

/// `send --wait` without `--timeout`: 9 minutes, inside the 10 minutes a Bash tool call may take.
pub const DEFAULT_WAIT_MS: u64 = 9 * 60 * 1000;

/// A duration: a number with `ms`, `s`, `m` or `h` (a bare number is seconds).
pub fn parse_duration(text: &str) -> Option<u64> {
    let text = text.trim();
    let split = text
        .find(|c: char| !c.is_ascii_digit() && c != '.')
        .unwrap_or(text.len());
    let (number, unit) = text.split_at(split);
    let number: f64 = number
        .parse()
        .ok()
        .filter(|number: &f64| number.is_finite() && *number >= 0.0)?;
    let factor = match unit.trim() {
        "ms" => 1.0,
        "" | "s" | "sec" => 1000.0,
        "m" | "min" => 60_000.0,
        "h" => 3_600_000.0,
        _ => return None,
    };
    let ms = (number * factor).round();
    // Bounded above, so the cast is exact enough and never wraps.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let ms = ms.min(1e15) as u64;
    Some(ms)
}

/// Where a plan's JSON comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlanSource {
    Stdin,
    File(String),
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
    pub prompt: Option<TextSource>,
    pub draft: bool,
    pub agent: Option<ProgramKey>,
    pub title: Option<String>,
    pub task: Option<String>,
    /// A session ID (or a prefix of one).
    pub parent: Option<String>,
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
    scan_args(args, options, usage, false)
}

/// [`scan`] for a command whose positionals are text: an unknown option may be text that starts with `-`, so its
/// error says how to pass that.
fn scan_text(args: &[String], options: &[(&'static str, Kind)], usage: &str) -> Result<Scanned, DhError> {
    scan_args(args, options, usage, true)
}

fn scan_args(args: &[String], options: &[(&'static str, Kind)], usage: &str, text: bool) -> Result<Scanned, DhError> {
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
            let hint = if text {
                " (options go anywhere; text that starts with - goes after --, or comes from stdin with -)"
            } else {
                ""
            };
            return Err(usage_error(&format!("Unknown option: {name}{hint}"), usage));
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

/// Positionals joined by spaces (`step add Write the tests`), which must not be blank. A lone `-` is refused: this
/// text is not read from stdin (`instead` says where long text goes).
fn words(scanned: &Scanned, what: &str, command: &str, usage: &str, instead: &str) -> Result<String, DhError> {
    let text = scanned.positionals.join(" ");
    if text.trim().is_empty() {
        return Err(usage_error(&format!("deckhand {command} needs {what}"), usage));
    }
    if text == "-" {
        return Err(usage_error(
            &format!("deckhand {command} takes {what} itself, not - (stdin); {instead}"),
            usage,
        ));
    }
    Ok(text)
}

/// Positionals joined by spaces, or `-`: stdin. Blank is refused.
fn long_text(scanned: &Scanned, what: &str, command: &str, usage: &str) -> Result<TextSource, DhError> {
    message_text(&scanned.positionals, what, command, usage)
}

/// An option's long text (`--prompt`, `--details`): given, or `-` (stdin). Blank is refused, never taken as none.
fn option_text(scanned: &Scanned, option: &str, usage: &str) -> Result<Option<TextSource>, DhError> {
    match scanned.value(option) {
        None => Ok(None),
        Some(value) if value == "-" => Ok(Some(TextSource::Stdin)),
        Some(value) if value.trim().is_empty() => Err(usage_error(
            &format!("{option} is empty: give the text, or - to read it from stdin"),
            usage,
        )),
        Some(value) => Ok(Some(TextSource::Given(value))),
    }
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
        "ls" => {
            let scanned = scan(rest, &[("--all", Kind::Switch)], LS_USAGE)?;
            if scanned.help {
                return Ok(help(LS_USAGE));
            }
            none(&scanned, LS_USAGE)?;
            Ok(Invocation {
                action: Action::Ls {
                    all: scanned.switch("--all"),
                },
                json: scanned.json,
            })
        }
        "guide" => simple(rest, GUIDE_USAGE, Action::Guide),
        "task" => parse_task(rest),
        "step" => parse_step(rest),
        "note" => parse_note(rest),
        "send" => parse_send(rest),
        "reply" => parse_reply(rest),
        "plan" => parse_plan(rest),
        "do" => {
            let scanned = scan_text(rest, &[("--title", Kind::Value)], DO_USAGE)?;
            if scanned.help {
                return Ok(help(DO_USAGE));
            }
            Ok(Invocation {
                action: Action::Do {
                    idea: long_text(&scanned, "an idea", "do", DO_USAGE)?,
                    title: scanned.value("--title"),
                },
                json: scanned.json,
            })
        }
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
        ("--parent", Kind::Value),
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
    let title = match scanned.value("--title") {
        Some(title) if title.trim().is_empty() => return Err(usage_error("--title is empty", NEW_USAGE)),
        Some(title) if title == "-" => {
            return Err(usage_error(
                "--title takes the name itself, not - (stdin); only --prompt reads -",
                NEW_USAGE,
            ));
        }
        title => title,
    };
    let new = NewArgs {
        prompt: option_text(&scanned, "--prompt", NEW_USAGE)?,
        draft: scanned.switch("--draft"),
        agent: scanned.value("--agent").as_deref().map(parse_agent).transpose()?,
        title,
        task: scanned.value("--task"),
        parent: scanned.value("--parent").filter(|parent| !parent.is_empty()),
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
    if new.parent.is_some() && new.top_level {
        return Err(usage_error("--parent and --top-level cannot be combined", NEW_USAGE));
    }
    if new.parent.is_some() && new.fork {
        return Err(usage_error("--parent and --fork cannot be combined", NEW_USAGE));
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
    let scanned = scan_args(args, options, usage, sub == Some("add"))?;
    if scanned.help || matches!(sub, None | Some("--help" | "-h")) {
        return Ok(help(usage));
    }
    let action = match sub {
        Some("list") => {
            none(&scanned, usage)?;
            Action::TaskList
        }
        Some("add") => Action::TaskAdd {
            title: words(&scanned, "a title", "task add", usage, "long text goes in --details -")?,
            details: option_text(&scanned, "--details", usage)?,
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

fn parse_plan(rest: &[String]) -> Result<Invocation, DhError> {
    let usage = PLAN_USAGE;
    let sub = rest.first().map(String::as_str);
    let scanned = scan(rest.get(1..).unwrap_or_default(), &[], usage)?;
    if scanned.help || matches!(sub, None | Some("--help" | "-h")) {
        return Ok(help(usage));
    }
    let source = match scanned.positionals.as_slice() {
        [] => PlanSource::Stdin,
        [only] if only == "-" => PlanSource::Stdin,
        [only] if !only.trim().is_empty() => PlanSource::File(only.clone()),
        [_] => return Err(usage_error("deckhand plan needs a file, or - for stdin", usage)),
        [_, extra, ..] => return Err(usage_error(&format!("Unexpected argument: {extra}"), usage)),
    };
    let action = match sub {
        Some("check") => Action::PlanCheck(source),
        Some("apply") => Action::PlanApply(source),
        Some(other) => return Err(usage_error(&format!("Unknown plan command: {other}"), usage)),
        None => return Ok(help(usage)),
    };
    Ok(Invocation {
        action,
        json: scanned.json,
    })
}

fn parse_step(rest: &[String]) -> Result<Invocation, DhError> {
    let usage = STEP_USAGE;
    let sub = rest.first().map(String::as_str);
    let scanned = scan_args(rest.get(1..).unwrap_or_default(), &[], usage, sub == Some("add"))?;
    if scanned.help || matches!(sub, None | Some("--help" | "-h")) {
        return Ok(help(usage));
    }
    let action = match sub {
        Some("add") => Action::StepAdd(words(
            &scanned,
            "a step",
            "step add",
            usage,
            "a step is one line, and long text goes in deckhand note add -",
        )?),
        Some("done") => Action::StepDone(one(&scanned, "a step ID", "step done", usage)?),
        Some("reopen") => Action::StepReopen(one(&scanned, "a step ID", "step reopen", usage)?),
        Some("list") => {
            none(&scanned, usage)?;
            Action::StepList
        }
        Some(other) => return Err(usage_error(&format!("Unknown step command: {other}"), usage)),
        None => return Ok(help(usage)),
    };
    Ok(Invocation {
        action,
        json: scanned.json,
    })
}

/// A message's words after its first positional (`-`: stdin).
fn message_text(rest: &[String], what: &str, command: &str, usage: &str) -> Result<TextSource, DhError> {
    match rest {
        [only] if only == "-" => Ok(TextSource::Stdin),
        _ => {
            let text = rest.join(" ");
            if text.trim().is_empty() {
                return Err(usage_error(&format!("deckhand {command} needs {what}"), usage));
            }
            Ok(TextSource::Given(text))
        }
    }
}

fn parse_send(rest: &[String]) -> Result<Invocation, DhError> {
    let usage = SEND_USAGE;
    let options = [
        ("--wait", Kind::Switch),
        ("--timeout", Kind::Value),
        ("--cancel", Kind::Value),
    ];
    let scanned = scan_text(rest, &options, usage)?;
    if scanned.help || rest.is_empty() {
        return Ok(help(usage));
    }
    if let Some(id) = scanned.value("--cancel") {
        if scanned.switch("--wait") || scanned.value("--timeout").is_some() {
            return Err(usage_error("--cancel takes no --wait or --timeout", usage));
        }
        none(&scanned, usage)?;
        if id.trim().is_empty() {
            return Err(usage_error("--cancel needs a message ID", usage));
        }
        return Ok(Invocation {
            action: Action::CancelMessage(id),
            json: scanned.json,
        });
    }
    let timeout = match scanned.value("--timeout") {
        None => None,
        Some(_) if !scanned.switch("--wait") => {
            return Err(usage_error("--timeout applies only with --wait", usage));
        }
        Some(text) => Some(
            parse_duration(&text)
                .ok_or_else(|| usage_error(&format!("Unknown duration: {text} (e.g. 90s, 5m, 1h)"), usage))?,
        ),
    };
    let Some((to, words)) = scanned
        .positionals
        .split_first()
        .filter(|(to, _)| !to.trim().is_empty())
    else {
        return Err(usage_error("deckhand send needs a session ID and a message", usage));
    };
    let text = message_text(words, "a message", "send", usage)?;
    Ok(Invocation {
        action: Action::Send(SendArgs {
            to: to.clone(),
            text,
            wait: scanned.switch("--wait").then(|| timeout.unwrap_or(DEFAULT_WAIT_MS)),
        }),
        json: scanned.json,
    })
}

fn parse_reply(rest: &[String]) -> Result<Invocation, DhError> {
    let usage = REPLY_USAGE;
    let scanned = scan_text(rest, &[], usage)?;
    if scanned.help || rest.is_empty() {
        return Ok(help(usage));
    }
    let Some((id, words)) = scanned
        .positionals
        .split_first()
        .filter(|(id, _)| !id.trim().is_empty())
    else {
        return Err(usage_error("deckhand reply needs a message ID and an answer", usage));
    };
    Ok(Invocation {
        action: Action::Reply {
            id: id.clone(),
            text: message_text(words, "an answer", "reply", usage)?,
        },
        json: scanned.json,
    })
}

fn parse_note(rest: &[String]) -> Result<Invocation, DhError> {
    let usage = NOTE_USAGE;
    let sub = rest.first().map(String::as_str);
    let scanned = scan_args(rest.get(1..).unwrap_or_default(), &[], usage, sub == Some("add"))?;
    if scanned.help || matches!(sub, None | Some("--help" | "-h")) {
        return Ok(help(usage));
    }
    match sub {
        Some("add") => Ok(Invocation {
            action: Action::NoteAdd(long_text(&scanned, "a text", "note add", usage)?),
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
            "--parent",
            "9f8e",
            "--worktree",
            "new",
            "--base=main",
            "--flat",
        ]);
        assert_eq!(
            all,
            NewArgs {
                prompt: Some(TextSource::Given("Fix the login bug.\nRun the tests.".into())),
                draft: true,
                agent: Some(ProgramKey::Codex),
                title: Some("Login".into()),
                task: Some("ab12".into()),
                parent: Some("9f8e".into()),
                worktree: Some(WorktreeChoice::New),
                base: Some("main".into()),
                flat: true,
                top_level: false,
                fork: false,
            }
        );
        assert!(new(&["--fork"]).fork);
        assert!(new(&["--top-level"]).top_level);
    }

    #[test]
    fn a_prompt_may_start_with_a_dash_or_hold_internal_flags() {
        let prompt = |list: &[&str]| new(list).prompt;
        assert_eq!(
            prompt(&["--prompt", "--daemon"]),
            Some(TextSource::Given("--daemon".into()))
        );
        assert_eq!(prompt(&["--prompt=-x"]), Some(TextSource::Given("-x".into())));
    }

    #[test]
    fn new_prompt_dash_reads_the_prompt_from_stdin() {
        let mut action = parsed(&["new", "--title", "X", "--prompt", "-"]).action;
        let Action::New(args) = &action else { panic!() };
        assert_eq!(args.prompt, Some(TextSource::Stdin));
        assert_eq!(args.title.as_deref(), Some("X"));
        let text = action.stdin_text().expect("reads stdin");
        assert_eq!((text.what, text.form), ("the prompt", "deckhand new --prompt -"));
        assert_eq!(new(&["--prompt=-"]).prompt, Some(TextSource::Stdin));
        // A given prompt reads nothing.
        assert!(parsed(&["new", "--prompt", "Fix it"]).action.stdin_text().is_none());
    }

    #[test]
    fn a_blank_prompt_title_or_details_is_refused_not_dropped() {
        assert!(refused(&["new", "--prompt", "  "]).starts_with("--prompt is empty: give the text, or - to read it"));
        assert!(refused(&["new", "--prompt="]).starts_with("--prompt is empty"));
        assert!(refused(&["new", "--title", " "]).starts_with("--title is empty\nUsage: deckhand new"));
        assert!(refused(&["task", "add", "Docs", "--details", "\n"]).starts_with("--details is empty"));
    }

    #[test]
    fn a_dash_where_stdin_is_not_read_is_refused() {
        assert!(
            refused(&["new", "--title", "-", "--prompt", "Fix it"])
                .starts_with("--title takes the name itself, not - (stdin); only --prompt reads -")
        );
        assert!(
            refused(&["task", "add", "-"])
                .starts_with("deckhand task add takes a title itself, not - (stdin); long text goes in --details -")
        );
        assert!(refused(&["step", "add", "-"]).starts_with(
            "deckhand step add takes a step itself, not - (stdin); a step is one line, and long text goes in \
             deckhand note add -"
        ));
        // A dash inside the words is text.
        assert_eq!(
            parsed(&["step", "add", "Ship", "-", "then", "tag"]).action,
            Action::StepAdd("Ship - then tag".into())
        );
    }

    #[test]
    fn every_long_text_reads_stdin_for_a_dash() {
        for (list, what) in [
            (&["new", "--prompt", "-"][..], "the prompt"),
            (&["task", "add", "Docs", "--details", "-"], "the details"),
            (&["note", "add", "-"], "the note"),
            (&["send", "1a2b", "-"], "the message"),
            (&["reply", "7a3f", "-"], "the answer"),
            (&["do", "-"], "the idea"),
        ] {
            let mut action = parsed(list).action;
            assert_eq!(action.stdin_text().map(|text| text.what), Some(what), "{list:?}");
        }
        assert_eq!(
            parsed(&["task", "add", "Docs", "--details", "-"]).action,
            Action::TaskAdd {
                title: "Docs".into(),
                details: Some(TextSource::Stdin)
            }
        );
        assert_eq!(parsed(&["note", "add", "-"]).action, Action::NoteAdd(TextSource::Stdin));
        assert_eq!(
            parsed(&["do", "-"]).action,
            Action::Do {
                idea: TextSource::Stdin,
                title: None
            }
        );
        // A name and an idea read from stdin go together.
        assert_eq!(
            parsed(&["do", "--title", "Auth tokens", "-"]).action,
            Action::Do {
                idea: TextSource::Stdin,
                title: Some("Auth tokens".into())
            }
        );
    }

    #[test]
    fn an_unknown_option_among_text_says_how_to_pass_text_that_starts_with_a_dash() {
        let hint = "(options go anywhere; text that starts with - goes after --, or comes from stdin with -)";
        assert!(refused(&["send", "1a2b", "hello", "--wiat"]).starts_with(&format!("Unknown option: --wiat {hint}")));
        assert!(refused(&["reply", "7a3f", "--verbose", "is", "broken"]).starts_with("Unknown option: --verbose ("));
        assert!(refused(&["note", "add", "-x"]).starts_with("Unknown option: -x ("));
        assert!(refused(&["task", "add", "Fix", "--force"]).starts_with("Unknown option: --force ("));
        // Commands without text keep the plain error.
        assert!(refused(&["new", "--yes"]).starts_with("Unknown option: --yes\n"));
        assert!(refused(&["task", "done", "a", "--force"]).starts_with("Unknown option: --force\n"));
        // After --, it is text.
        assert_eq!(
            parsed(&["send", "1a2b", "--", "--verbose", "is", "broken"]).action,
            Action::Send(SendArgs {
                to: "1a2b".into(),
                text: TextSource::Given("--verbose is broken".into()),
                wait: None,
            })
        );
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
        assert!(refused(&["new", "--parent", "a", "--top-level"]).starts_with("--parent and --top-level cannot"));
        assert!(refused(&["new", "--parent", "a", "--fork"]).starts_with("--parent and --fork cannot"));
        // A session may start on a line wherever it runs.
        assert_eq!(
            parsed(&["new", "--parent", "a", "--task", "t", "--worktree", "same"]).action,
            Action::New(NewArgs {
                parent: Some("a".into()),
                task: Some("t".into()),
                worktree: Some(WorktreeChoice::Same),
                ..NewArgs::default()
            })
        );
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
            (&["step"], STEP_USAGE),
            (&["step", "done", "--help"], STEP_USAGE),
            (&["note"], NOTE_USAGE),
            (&["note", "add", "-h"], NOTE_USAGE),
            (&["kill", "--help"], KILL_USAGE),
            (&["remove", "--help"], REMOVE_USAGE),
            (&["guide", "--help"], GUIDE_USAGE),
            (&["plan"], PLAN_USAGE),
            (&["plan", "apply", "--help"], PLAN_USAGE),
            (&["do", "--help"], DO_USAGE),
            (&["send"], SEND_USAGE),
            (&["send", "1a2b", "--help"], SEND_USAGE),
            (&["reply"], REPLY_USAGE),
            (&["reply", "-h"], REPLY_USAGE),
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
                details: Some(TextSource::Given("For the CLI".into()))
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
    fn step_commands_take_a_text_or_an_id() {
        assert_eq!(
            parsed(&["step", "add", "Write", "the", "tests"]).action,
            Action::StepAdd("Write the tests".into())
        );
        assert_eq!(
            parsed(&["step", "done", "ab12"]).action,
            Action::StepDone("ab12".into())
        );
        assert_eq!(
            parsed(&["step", "reopen", "ab12"]).action,
            Action::StepReopen("ab12".into())
        );
        assert_eq!(
            parsed(&["step", "list", "--json"]),
            Invocation {
                action: Action::StepList,
                json: true
            }
        );
        assert!(refused(&["step", "add"]).starts_with("deckhand step add needs a step"));
        assert!(refused(&["step", "list", "x"]).starts_with("Unexpected argument: x"));
        assert!(refused(&["step", "tick", "a"]).starts_with("Unknown step command: tick"));
    }

    #[test]
    fn note_add_joins_its_words_and_other_commands_take_one_id() {
        assert_eq!(
            parsed(&["note", "add", "Check", "the", "migration"]).action,
            Action::NoteAdd(TextSource::Given("Check the migration".into()))
        );
        assert!(refused(&["note", "add", " "]).starts_with("deckhand note add needs a text"));
        assert!(refused(&["note", "list"]).starts_with("Unknown note command: list"));
        assert_eq!(
            parsed(&["kill", "1f2e", "--json"]),
            Invocation {
                action: Action::Kill("1f2e".into()),
                json: true
            }
        );
        assert_eq!(parsed(&["remove", "1f2e"]).action, Action::Remove("1f2e".into()));
        assert_eq!(parsed(&["ls"]).action, Action::Ls { all: false });
        assert_eq!(
            parsed(&["ls", "--all", "--json"]),
            Invocation {
                action: Action::Ls { all: true },
                json: true
            }
        );
        assert!(refused(&["ls", "all"]).starts_with("Unexpected argument: all"));
        assert_eq!(parsed(&["guide"]).action, Action::Guide);
    }

    #[test]
    fn the_guide_names_every_command_and_option_of_new() {
        let guide = deckhand_core::agent_guide::GUIDE;
        for word in AGENT_WORDS.iter().filter(|word| **word != "guide") {
            assert!(guide.contains(&format!("deckhand {word}")), "{word}");
        }
        for listed in [
            "task list",
            "task add",
            "task done|reopen",
            "task move",
            "step add",
            "step done|reopen",
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
    fn plan_reads_its_json_from_a_file_or_stdin_and_do_takes_the_idea() {
        assert_eq!(parsed(&["plan", "check"]).action, Action::PlanCheck(PlanSource::Stdin));
        assert_eq!(
            parsed(&["plan", "apply", "-"]).action,
            Action::PlanApply(PlanSource::Stdin)
        );
        assert_eq!(
            parsed(&["plan", "apply", "plan.json", "--json"]),
            Invocation {
                action: Action::PlanApply(PlanSource::File("plan.json".into())),
                json: true
            }
        );
        assert!(refused(&["plan", "apply", "a", "b"]).starts_with("Unexpected argument: b"));
        assert!(refused(&["plan", "run"]).starts_with("Unknown plan command: run"));
        assert_eq!(
            parsed(&["do", "Split", "the", "auth", "rework"]).action,
            Action::Do {
                idea: TextSource::Given("Split the auth rework".into()),
                title: None
            }
        );
        assert!(refused(&["do"]).starts_with("deckhand do needs an idea"));
        // --title names the orchestrator, before or after the idea, its value as given (the daemon cleans it).
        for args in [
            &["do", "--title", "Auth tokens", "Store", "tokens", "encrypted"][..],
            &["do", "Store", "tokens", "encrypted", "--title=Auth tokens"],
        ] {
            assert_eq!(
                parsed(args).action,
                Action::Do {
                    idea: TextSource::Given("Store tokens encrypted".into()),
                    title: Some("Auth tokens".into())
                }
            );
        }
        assert!(refused(&["do", "Store tokens", "--title"]).starts_with("--title needs a value"));
        assert!(refused(&["do", "--title", "Auth tokens"]).starts_with("deckhand do needs an idea"));
        // The orchestrator's guide names plan; the agents' guide leaves out what is not theirs.
        assert!(deckhand_core::orchestrator_guide::ORCHESTRATOR_GUIDE.contains("deckhand plan apply"));
    }

    #[test]
    fn send_takes_a_session_a_message_and_how_long_to_wait() {
        assert_eq!(
            parsed(&["send", "1a2b", "Which", "store?"]).action,
            Action::Send(SendArgs {
                to: "1a2b".into(),
                text: TextSource::Given("Which store?".into()),
                wait: None,
            })
        );
        assert_eq!(
            parsed(&["send", "1a2b", "-", "--wait", "--json"]),
            Invocation {
                action: Action::Send(SendArgs {
                    to: "1a2b".into(),
                    text: TextSource::Stdin,
                    wait: Some(DEFAULT_WAIT_MS),
                }),
                json: true
            }
        );
        let Action::Send(args) = parsed(&["send", "1a2b", "hi", "--wait", "--timeout", "90s"]).action else {
            panic!()
        };
        assert_eq!(args.wait, Some(90_000));
        assert_eq!(
            parsed(&["send", "--cancel", "7a3f"]).action,
            Action::CancelMessage("7a3f".into())
        );
        assert!(refused(&["send", "1a2b"]).starts_with("deckhand send needs a message"));
        assert!(refused(&["send", "1a2b", "hi", "--timeout", "5m"]).starts_with("--timeout applies only with --wait"));
        assert!(refused(&["send", "1a2b", "hi", "--wait", "--timeout", "soon"]).starts_with("Unknown duration: soon"));
        assert!(refused(&["send", "--cancel", "7a3f", "--wait"]).starts_with("--cancel takes no --wait"));
        assert!(refused(&["send", "--cancel", "7a3f", "extra"]).starts_with("Unexpected argument: extra"));
        assert_eq!(
            parsed(&["reply", "7a3f", "The", "keychain"]).action,
            Action::Reply {
                id: "7a3f".into(),
                text: TextSource::Given("The keychain".into())
            }
        );
        assert_eq!(
            parsed(&["reply", "7a3f", "-"]).action,
            Action::Reply {
                id: "7a3f".into(),
                text: TextSource::Stdin
            }
        );
        assert!(refused(&["reply", "7a3f"]).starts_with("deckhand reply needs an answer"));
    }

    #[test]
    fn durations_take_a_unit_and_a_bare_number_is_seconds() {
        assert_eq!(parse_duration("90"), Some(90_000));
        assert_eq!(parse_duration("90s"), Some(90_000));
        assert_eq!(parse_duration("5m"), Some(300_000));
        assert_eq!(parse_duration("1.5h"), Some(5_400_000));
        assert_eq!(parse_duration("250ms"), Some(250));
        assert_eq!(parse_duration("5 min"), Some(300_000));
        assert_eq!(parse_duration("-5s"), None);
        assert_eq!(parse_duration("5d"), None);
        assert_eq!(parse_duration(""), None);
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
