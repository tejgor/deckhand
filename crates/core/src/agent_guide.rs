//! What agents Deckhand launches are told about the `deckhand` commands (not in the TS).
//!
//! Every session carries [`TRIGGER`] (or [`SKILL_DESCRIPTION`], for an agent that loads skills lazily, or
//! [`CODEX_TRIGGER`], for a Codex given the `deckhand` MCP tool), so it stays a line or two; the full [`GUIDE`] is
//! printed by `deckhand guide` (and is the skill's body) and only costs context in the turn the user asks for a
//! Deckhand action.

/// The skill's name, for agents that load skills lazily (Claude: `--plugin-dir`).
pub const SKILL_NAME: &str = "deckhand";

/// The skill's description: all an agent with lazily loaded skills keeps in context until it uses the skill.
pub const SKILL_DESCRIPTION: &str = "Deckhand actions in this Deckhand session: start sessions or sub-sessions, \
message them, add tasks to the backlog, add steps or notes to this session's line, list or tick tasks. Only when the \
user asks for one.";

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

/// The most [`GUIDE`] may be, in bytes (about 530 tokens): it is read only when an agent takes a Deckhand action (or
/// answers a message), so it can be clear rather than terse, with exact forms rather than pointers; details belong in
/// each command's `--help`.
pub const GUIDE_BUDGET: usize = 2200;

/// `deckhand guide`: the full guide, and the skill's body.
pub const GUIDE: &str = "\
# deckhand: act for this Deckhand session, only when the user asks

Take Deckhand actions only when the user asks, with one exception: when a message asks you for an answer, answer it
with deckhand reply. Never start or end sessions or move tasks unasked. Merges and worktree deletions stay with the
user.

deckhand new --title <t> --prompt <brief> [--agent claude|codex|pi] [--task <id>] [--parent <id>]
  [--worktree new|same|none|<path>] [--base <branch>] [--flat] [--top-level] [--fork] [--draft]
  Starts a sub-session of this one in a new worktree, stacked on this branch (--flat: not). --worktree same: this
  worktree; none: the main checkout. --top-level: a sibling; --parent: under a session below you. Its
  prompt is sent at once (--draft: typed for the user to send). It knows nothing of this chat unless --fork: brief
  it fully (context, constraints, what done is). --task: it works on that line, ticked when merged or done.
  Always pass --title: 2-4 words naming the work (\"Fix login redirect\"), not a sentence.
deckhand ls  your tree, who you can message (* is you)
deckhand kill <id>  ends a session below you; deckhand remove <id> takes an ended one off the list

deckhand send <id> <message> [--wait]  messages a session in your tree (deckhand ls shows them),
  typed in once it is idle. --wait prints the first reply (Bash timeout 600000) or why none came. Later replies
  come as messages.
deckhand reply <message id> <answer>  answers a message sent to you (its header has the ID)

deckhand task list | task add <title> [--details <text>] | task done|reopen <id> | task move <id> --to backlog|<session>|<line>
  Told to park something? task add it.
deckhand step add <text> | step done|reopen <id> | step list  your steps
deckhand note add <text>  findings for your line's note

Long text (a prompt, message, answer, note or --details): pass - and pipe it in with a quoted heredoc, EOF
alone on its last line:
deckhand new --prompt - <<'EOF'
<brief>
EOF
IDs: any unique prefix. --json anywhere; <command> --help for more.
As tool args, pass the words after `deckhand`, e.g. [\"new\", \"--prompt\", \"<brief>\"]; no quoting needed. No
stdin: long text is one word, never -.
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

    #[test]
    fn the_guide_makes_answering_a_message_the_one_exception() {
        assert!(GUIDE.contains("only when the user asks, with one exception: when a message asks you for an answer"));
        assert!(GUIDE.contains("deckhand reply <message id> <answer>"));
        assert!(GUIDE.contains("deckhand send <id> <message> [--wait]"));
        assert!(GUIDE.contains("Bash timeout 600000"));
    }

    #[test]
    fn the_guide_shows_the_exact_form_for_long_text_once_for_every_command() {
        assert!(GUIDE.contains("Long text (a prompt, message, answer, note or --details): pass - and pipe it in"));
        // The heredoc as it is typed: its terminator alone at the start of a line, or the shell never ends it.
        assert!(GUIDE.contains("\ndeckhand new --prompt - <<'EOF'\n<brief>\nEOF\n"));
        assert!(GUIDE.contains("No\nstdin: long text is one word, never -."));
        // No pointer in place of the form.
        assert!(!GUIDE.contains("(new --help)"));
    }

    #[test]
    fn the_guide_asks_for_a_short_title_naming_the_work_on_every_new_session() {
        assert!(GUIDE.contains("deckhand new --title <t> --prompt <brief>"));
        assert!(
            GUIDE.contains("Always pass --title: 2-4 words naming the work (\"Fix login redirect\"), not a sentence.")
        );
    }

    #[test]
    fn the_guide_says_ls_shows_who_a_session_can_message() {
        assert!(GUIDE.contains("deckhand ls  your tree, who you can message"));
        assert!(GUIDE.contains("messages a session in your tree (deckhand ls shows them)"));
    }
}
