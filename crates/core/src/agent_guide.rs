//! What agents Deckhand launches are told about the `deckhand` commands (not in the TS).
//!
//! Every session carries [`TRIGGER`] (or [`SKILL_DESCRIPTION`], for an agent that loads skills lazily, or
//! [`CODEX_TRIGGER`], for a Codex given the `deckhand` MCP tool), so it stays a line or two; the full [`GUIDE`] is
//! printed by `deckhand guide` (and is the skill's body) and only costs context in the turn the user asks for a
//! Deckhand action.

/// The skill's name, for agents that load skills lazily (Claude: `--plugin-dir`).
pub const SKILL_NAME: &str = "deckhand";

/// The skill's description: all an agent with lazily loaded skills keeps in context until it uses the skill.
pub const SKILL_DESCRIPTION: &str = "Deckhand actions in this Deckhand session: start sessions or sub-sessions, add \
tasks to the backlog, add steps or notes to this session's line, list or tick tasks. Only when the user asks for one.";

/// The always-loaded instruction for agents without lazily loaded skills.
pub const TRIGGER: &str = "This is a Deckhand session. Only when the user asks for a Deckhand action (start a \
session, add a task, a step or a note), run `deckhand guide` once per conversation, unless it is still in context.";

/// The always-loaded instruction for Codex when it has the `deckhand` MCP tool (`deckhand mcp`), whose sandbox keeps
/// its shell from reaching the daemon.
pub const CODEX_TRIGGER: &str = "This is a Deckhand session. Only when the user asks for a Deckhand action (start a \
session, add a task, a step or a note), call the `deckhand` tool with args [\"guide\"] once per conversation, unless \
it is still in context.";

/// The most an always-loaded text may be, in bytes (about 60 tokens).
pub const ALWAYS_LOADED_BUDGET: usize = 240;

/// The most [`GUIDE`] may be, in bytes (about 330 tokens): it is read whenever an agent takes a Deckhand action, so
/// details belong in each command's `--help`.
pub const GUIDE_BUDGET: usize = 1400;

/// `deckhand guide`: the full guide, and the skill's body.
pub const GUIDE: &str = "\
# deckhand: act for this Deckhand session, only when the user asks

Never start sessions, move tasks or end sessions unasked. Merges and worktree deletions stay with the user.

deckhand new --prompt <brief> [--title <t>] [--agent claude|codex|pi] [--task <id>] [--parent <id>]
  [--worktree new|same|none|<path>] [--base <branch>] [--flat] [--top-level] [--fork] [--draft]
  Default: a sub-session in a new worktree stacked on this branch (--flat: not); same: this worktree; none: the
  main checkout; --top-level: a sibling; --parent: under one you started. Sent at once (--draft: typed). Brief it
  fully (context, constraints, what done is): it knows nothing of this chat unless --fork. Long prompts: quoted
  heredoc (new --help). --task: on that line, ticked when merged or done.
deckhand task list | task add <title> [--details <text>] | task done|reopen <id> | task move <id> --to backlog|<session>
  Told to park something? task add it.
deckhand step add <text> | step done|reopen <id> | step list  your steps
deckhand note add <text>  findings for your line's note
deckhand ls               sessions (* is this one)
deckhand kill <id>        end a session you started
deckhand remove <id>      take it off the list

IDs: any unique prefix. --json anywhere; <command> --help for more.
As tool args, pass the words after `deckhand`, e.g. [\"new\", \"--prompt\", \"<brief>\"]; no quoting needed.
";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_always_loaded_texts_stay_within_the_budget() {
        for text in [TRIGGER, SKILL_DESCRIPTION, CODEX_TRIGGER] {
            assert!(text.len() <= ALWAYS_LOADED_BUDGET, "{} bytes: {text}", text.len());
        }
    }

    #[test]
    fn the_guide_stays_within_its_budget() {
        assert!(GUIDE.len() <= GUIDE_BUDGET, "{} bytes", GUIDE.len());
    }

    #[test]
    fn the_trigger_points_at_the_guide() {
        assert!(TRIGGER.contains("deckhand guide"));
        assert!(CODEX_TRIGGER.contains("`deckhand` tool with args [\"guide\"] once per conversation"));
        assert!(TRIGGER.contains("`deckhand guide` once per conversation"));
        assert!(GUIDE.contains("As tool args, pass the words after `deckhand`"));
        assert!(GUIDE.starts_with("# deckhand: "));
    }
}
