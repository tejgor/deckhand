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
tasks to the backlog, add steps to the note, list or tick tasks. Only when the user asks for one.";

/// The always-loaded instruction for agents without lazily loaded skills.
pub const TRIGGER: &str = "This is a Deckhand session. Only when the user asks for a Deckhand action (start a \
session, add a task or a note step), run `deckhand guide` once per conversation, unless it is still in context.";

/// The always-loaded instruction for Codex when it has the `deckhand` MCP tool (`deckhand mcp`), whose sandbox keeps
/// its shell from reaching the daemon.
pub const CODEX_TRIGGER: &str = "This is a Deckhand session. Only when the user asks for a Deckhand action (start a \
session, add a task or a note step), call the `deckhand` tool with args [\"guide\"] once per conversation, unless \
it is still in context.";

/// The most an always-loaded text may be, in bytes (about 60 tokens).
pub const ALWAYS_LOADED_BUDGET: usize = 240;

/// The most [`GUIDE`] may be, in bytes (about 330 tokens): it is read whenever an agent takes a Deckhand action, so
/// details belong in each command's `--help`.
pub const GUIDE_BUDGET: usize = 1400;

/// `deckhand guide`: the full guide, and the skill's body.
pub const GUIDE: &str = "\
# deckhand: act for this Deckhand session, only when the user asks

Never start sessions, move tasks or end sessions on your own. Merging and deleting worktrees stay with the user.

deckhand new --prompt <brief> [--title <t>] [--agent claude|codex|pi] [--task <id>]
  [--worktree new|same|none|<path>] [--base <branch>] [--top-level] [--fork] [--draft]
  Default: a sub-session in a new worktree; same: this worktree; none: the main checkout; --top-level: a sibling.
  The prompt is sent (--draft: typed, not sent). Brief it fully (context, constraints, what done means): it knows
  nothing of this conversation unless --fork. Shell: long prompts via a quoted heredoc (new --help).
  --task: works on that task, replacing its text; ticked when merged or done; top-level unless in a new worktree.
deckhand task list | task add <title> [--details <text>] | task done|reopen <id> | task move <id> --to backlog|<session>
  Told to park something? task add it rather than doing it.
deckhand note add <item>      a step in this session's note
deckhand ls                   sessions (* is this one)
deckhand kill <id>            end a sub-session this session started
deckhand remove <id>          then take it off the list

IDs: any unique prefix. --json on any command; <command> --help for details.
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
