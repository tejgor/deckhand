//! What agents Deckhand launches are told about the `deckhand` commands (not in the TS).
//!
//! Every session carries [`TRIGGER`] (or [`SKILL_DESCRIPTION`], for an agent that loads skills lazily), so it stays
//! a line or two; the full [`GUIDE`] is printed by `deckhand guide` (and is the skill's body) and only costs context
//! in the turn the user asks for a Deckhand action.

/// The skill's name, for agents that load skills lazily (Claude: `--plugin-dir`).
pub const SKILL_NAME: &str = "deckhand";

/// The skill's description: all an agent with lazily loaded skills keeps in context until it uses the skill.
pub const SKILL_DESCRIPTION: &str = "Deckhand actions in this Deckhand session: start sessions or sub-sessions, add \
tasks to the backlog, add steps to the note, list or tick tasks. Only when the user asks for one.";

/// The always-loaded instruction for agents without lazily loaded skills.
pub const TRIGGER: &str = "This is a Deckhand session. Only when the user asks for a Deckhand action (start a \
session, add a task or a note step), run `deckhand guide` first and follow it.";

/// The most an always-loaded text may be, in bytes (about 60 tokens).
pub const ALWAYS_LOADED_BUDGET: usize = 240;

/// `deckhand guide`: the full guide, and the skill's body.
pub const GUIDE: &str = "\
# Deckhand commands

You run inside a Deckhand session. The `deckhand` commands act for this session. Use them only when the user asks for
a Deckhand action; never start sessions, move tasks or end sessions on your own.

## Starting sessions

  deckhand new --prompt <text> [--title <t>] [--agent claude|codex|pi] [--task <id>]
               [--worktree new|same|none|<path>] [--base <branch>] [--top-level] [--fork] [--draft]

- By default the new session is a sub-session of this one, in a new worktree. `--worktree same` shares this
  session's worktree, `--worktree none` runs in the repository's checkout, `--top-level` makes it a sibling,
  `--fork` copies this conversation (same agent).
- The prompt is sent. Write it as a complete brief: the context, the constraints and what done means; the new agent
  knows nothing of this conversation (unless `--fork`). `--draft` types it without sending, for the user to review.
  A long prompt can come from a quoted heredoc:
    deckhand new --prompt \"$(cat <<'EOF'
    ...
    EOF
    )\"
- `--task <id>` starts the session on a task from `deckhand task list` (top-level unless in a new worktree); the
  prompt replaces the task's text, and the task is ticked when the work is merged or done.
- The name defaults to the task's title or the prompt's first line; `--title` sets it (at most 64 characters).

## Tasks and notes

  deckhand task list
  deckhand task add <title> [--details <text>]     (to the backlog)
  deckhand task done|reopen <id>
  deckhand task move <id> --to backlog|<session id>
  deckhand note add <item>                         (a checklist step in this session's note)

When the user says to park something for later, `task add` it rather than doing it. Moving a task onto a session
makes it the task of that session's work, or a step in its note when the work has a task already.

## Sessions

  deckhand ls                                      (* marks this session)
  deckhand kill <session id>                       (ends a running sub-session this session started)
  deckhand remove <session id>                     (removes an ended one from the list)

IDs may be shortened to any unique prefix, as `ls` and `task list` print them. Add `--json` to any command for
machine-readable output. `deckhand <command> --help` describes one command. Merging and deleting worktrees stay
with the user.
";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_always_loaded_texts_stay_within_the_budget() {
        for text in [TRIGGER, SKILL_DESCRIPTION] {
            assert!(text.len() <= ALWAYS_LOADED_BUDGET, "{} bytes: {text}", text.len());
        }
    }

    #[test]
    fn the_trigger_points_at_the_guide() {
        assert!(TRIGGER.contains("deckhand guide"));
        assert!(GUIDE.starts_with("# Deckhand commands"));
    }
}
