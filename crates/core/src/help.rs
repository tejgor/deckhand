//! Port of help.ts.
//!
//! The ? guide as data: short topics of key → what it does, with a few notes. Rendered by the help pane (topic list,
//! aligned key column, search). HELP_TOPICS was generated from src/help.ts (Node strips its types); U6 checks the
//! keymap against it.

use serde::Serialize;

/// TS: help.ts HelpLine.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum HelpLine {
    Key { keys: &'static str, text: &'static str },
    Note { note: &'static str },
    Heading { heading: &'static str },
}

/// TS: help.ts HelpTopic.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct HelpTopic {
    pub title: &'static str,
    pub lines: &'static [HelpLine],
}

/// TS: help.ts HELP_TOPICS.
pub static HELP_TOPICS: &[HelpTopic] = &[
    HelpTopic {
        title: "Start here",
        lines: &[
            HelpLine::Note {
                note: "The session list on the left, the pane on the right: → steps in, ← or Esc back out. Everywhere: Enter opens, Space toggles, E opens your editor; in lists a adds, x removes (on the session list: a Notes tab, x stop).",
            },
            HelpLine::Key {
                keys: "n",
                text: "New session: agent, name, Tab for the workspace (↑↓ a new worktree's base), Enter launches",
            },
            HelpLine::Key {
                keys: "j k  ↑↓",
                text: "Select a session (or type its number)",
            },
            HelpLine::Key {
                keys: "Enter  o",
                text: "Open it full screen: the agent, or what the tab shows (shell, Git, Dev); on Notes, Enter edits them here, o in vim",
            },
            HelpLine::Key {
                keys: "Ctrl+]",
                text: "Back to Deckhand from full screen (Ctrl+Space works too)",
            },
            HelpLine::Key {
                keys: "Tab  [ ]",
                text: "Next / previous tab: Preview, Terminal, Git, Dev, Notes (Shift+Tab goes back too)",
            },
            HelpLine::Key {
                keys: "b  W",
                text: "Tasks: the repository's task list (n starts a session from one) / Worktrees: what is merged, delete them",
            },
            HelpLine::Key {
                keys: "C  U",
                text: "Settings / Agents: versions and updates (Enter updates, r re-checks)",
            },
            HelpLine::Key {
                keys: "?",
                text: "This guide (↑↓ or 1-9 pick a topic, / searches, Esc closes)",
            },
            HelpLine::Key {
                keys: "q",
                text: "Quit the UI (agents and the daemon keep running); everywhere else Esc goes back",
            },
        ],
    },
    HelpTopic {
        title: "Sessions",
        lines: &[
            HelpLine::Key {
                keys: "N",
                text: "Sub-session of the selected one: clean, ⑂ a fork of it, or ↳ from its handoff (after H)",
            },
            HelpLine::Key {
                keys: "x  X",
                text: "Stop, asking first (and whether to delete its worktree) / force-stop at once; starting: cancel",
            },
            HelpLine::Key {
                keys: "s  S",
                text: "Resume an exited session (or retry a failed setup) / restart it with a fresh conversation",
            },
            HelpLine::Key {
                keys: "Space",
                text: "Mark done / not done (☑; any session, not the same as merged)",
            },
            HelpLine::Key {
                keys: "m  M",
                text: "Merge the worktree into this branch (see Git & PRs) / mark it merged without merging (✓)",
            },
            HelpLine::Key {
                keys: "Backspace  A",
                text: "Archive an exited session (on an archived one: remove it for good, asking first) / archive or unarchive any",
            },
            HelpLine::Key {
                keys: "H",
                text: "Export a Markdown handoff (notes, commits, changed files, diff stat) and open it",
            },
            HelpLine::Note {
                note: "A fork copies the conversation as saved then, not a turn in progress.",
            },
            HelpLine::Note {
                note: "Deleting a worktree on stop is blocked by uncommitted work or unpushed commits unless you type DELETE.",
            },
        ],
    },
    HelpTopic {
        title: "Sidebar",
        lines: &[
            HelpLine::Note {
                note: "A row: number, tree, status, title, then markers and the agent. Archived rows are dimmed (except in the archived view, which lists only them).",
            },
            HelpLine::Key {
                keys: "⠋ ● ◌ ○",
                text: "Starting or working · idle · running, activity unknown · exited",
            },
            HelpLine::Key {
                keys: "? ◆ ⌛",
                text: "Needs input · unread reply (attaching reads it) · rate-limited",
            },
            HelpLine::Key {
                keys: "!",
                text: "Before the title: failed (red) or interrupted (yellow)",
            },
            HelpLine::Key {
                keys: "▾ ▸",
                text: "Has sub-sessions: expanded / collapsed (c)",
            },
            HelpLine::Key {
                keys: "↳ ⑂",
                text: "Clean / forked sub-session",
            },
            HelpLine::Key {
                keys: "▶",
                text: "Dev runs in this worktree (on its first row)",
            },
            HelpLine::Key {
                keys: "▣ ✓ ☑",
                text: "Archived · merged · done (Space; its title muted)",
            },
            HelpLine::Key {
                keys: "! +N",
                text: "After the title: cleanup error · N hidden sub-sessions",
            },
            HelpLine::Key {
                keys: "✶ π ◇",
                text: "Agent: Claude · Pi · Codex",
            },
            HelpLine::Key {
                keys: "↑",
                text: "Runs an older agent version than installed: restart it (x, then s) to update",
            },
            HelpLine::Key {
                keys: "╎",
                text: "Shares the selected session's worktree: Terminal, Git, Dev",
            },
            HelpLine::Key {
                keys: "! N",
                text: "Header: N sessions need attention (! jumps to them)",
            },
            HelpLine::Note {
                note: "Below the list: the selected session's full title, agent · state · age (both versions when ↑), branch or main checkout, then ╎N (N others share the worktree) ▶ ✓ ☑2d (done 2 days ago) ▣, ☐ N open checklist items.",
            },
        ],
    },
    HelpTopic {
        title: "Find & organize",
        lines: &[
            HelpLine::Key {
                keys: "1-9  0",
                text: "Select by number (0 is 10; with more than 10, type the number: Enter or a short pause selects, Esc clears)",
            },
            HelpLine::Key {
                keys: "/",
                text: "Search titles, notes, tasks, agent, branch and path (Enter keeps it, Esc clears)",
            },
            HelpLine::Key {
                keys: "f",
                text: "Filter menu: a active · r running · ! attention · e exited · A archived · * all; f f back to active (the sidebar header names any other)",
            },
            HelpLine::Key {
                keys: "!",
                text: "Next session that needs attention: blocked ones first, then unread replies",
            },
            HelpLine::Key {
                keys: "J K",
                text: "Move the session down / up among its siblings",
            },
            HelpLine::Key {
                keys: "c",
                text: "Hide exited children → collapse → expand",
            },
            HelpLine::Key {
                keys: "< >",
                text: "Narrow / widen the sidebar",
            },
            HelpLine::Key {
                keys: "R",
                text: "Reload the list (it updates live; rarely needed)",
            },
        ],
    },
    HelpTopic {
        title: "Panes",
        lines: &[
            HelpLine::Key {
                keys: "p t g d a",
                text: "Jump to Preview, Terminal, Git, Dev, Notes (d twice quickly also starts or stops Dev)",
            },
            HelpLine::Key {
                keys: "→  l",
                text: "Step into the pane: scroll the preview (running), browse Git changes, edit the notes",
            },
            HelpLine::Key {
                keys: "←  h  Esc",
                text: "Step back out to the session list",
            },
            HelpLine::Key {
                keys: "v",
                text: "On Terminal: switch between the shell and the last action",
            },
            HelpLine::Key {
                keys: "E",
                text: "Open in Cursor / VS Code: the note on Notes, otherwise the session's folder",
            },
            HelpLine::Heading {
                heading: "Scrolling the preview (→)",
            },
            HelpLine::Key {
                keys: "j k  ↑↓",
                text: "Scroll (PgUp/PgDn a page, g/G top/bottom, the mouse wheel; + − change wheel speed)",
            },
            HelpLine::Note {
                note: "Agents ask for approvals in their own terminal: open it (Enter) to answer. Terminal, Git and Dev belong to the worktree: all its sessions share them, also after their agents exit.",
            },
        ],
    },
    HelpTopic {
        title: "Notes",
        lines: &[
            HelpLine::Note {
                note: "A worktree has one note, shared by its sessions; a main-checkout session has its own and the main checkout's. Autosaved Markdown; removed notes go to notes/trash.",
            },
            HelpLine::Key {
                keys: "Enter  o",
                text: "Edit here (Esc ends) / in vim: Ctrl+X, Ctrl+T, Ctrl+] back",
            },
            HelpLine::Key {
                keys: "Tab",
                text: "Main checkout: switch between this session's note and the shared one",
            },
            HelpLine::Key {
                keys: "Ctrl+X",
                text: "Check / uncheck the line's item, or make the line a - [ ] item",
            },
            HelpLine::Key {
                keys: "Ctrl+T",
                text: "New checklist item below (Enter on an item continues the list)",
            },
            HelpLine::Key {
                keys: "Alt+← →",
                text: "Jump a word (Ctrl+←→, Alt+B/F too); Alt+Backspace or Ctrl+W deletes one, Alt+Delete the next",
            },
            HelpLine::Key {
                keys: "Ctrl+A  E",
                text: "Start / end of the line (Home / End too; Ctrl+Home/End: the note; PgUp/PgDn a screen)",
            },
            HelpLine::Key {
                keys: "Ctrl+P",
                text: "Send the line's checklist item to Tasks (the note keeps a ↗ link)",
            },
            HelpLine::Key {
                keys: "E  Ctrl+O",
                text: "Open the note in Cursor / VS Code (Ctrl+O while editing)",
            },
            HelpLine::Note {
                note: "☐ N open below the sidebar counts open items of both notes. A worktree's note goes when its last session is removed (removing asks first; Enter sends open items to Tasks).",
            },
        ],
    },
    HelpTopic {
        title: "Tasks",
        lines: &[
            HelpLine::Note {
                note: "One list per repository. A worktree (or main-checkout session) has one task, ◆; its to-dos are its note's checklist, shown under it as steps (2/5).",
            },
            HelpLine::Key {
                keys: "b  v",
                text: "The board: a group per worktree with its task, backlog, done / only the selected session's worktree",
            },
            HelpLine::Key {
                keys: "a  A",
                text: "Add here (on a worktree: a step to its note) / a backlog task",
            },
            HelpLine::Key {
                keys: "Enter",
                text: "Edit it (Tab: details, Ctrl+S saves); on a step, open its note",
            },
            HelpLine::Key {
                keys: "n  w",
                text: "Start a session for it (↑↓ picks the base) / move it: backlog, its note, a worktree or session (its task, or a step in its note if it has one)",
            },
            HelpLine::Key {
                keys: "Space  x",
                text: "Tick or reopen (a step: in its note) / delete (x twice)",
            },
            HelpLine::Key {
                keys: "o  J K",
                text: "Open the session doing it / reorder (j k, g G move)",
            },
            HelpLine::Key {
                keys: "Tab  E",
                text: "Every note by worktree (a sends an item, A all its open ones, Enter opens it, f only open items) / the list in your editor",
            },
            HelpLine::Note {
                note: "Merging, or marking all its sessions done, ticks ◆ (undoing reopens it); work removed unmerged sends it back. m, x and W send the note's open items to the backlog too (Space keeps them).",
            },
        ],
    },
    HelpTopic {
        title: "Worktrees",
        lines: &[
            HelpLine::Note {
                note: "W lists every worktree of the repository (Deckhand's or not) by what to do with it: merged and safe to delete, merged with leftovers, in progress, idle (nothing running for 14 days), missing (directory gone), and the main checkout.",
            },
            HelpLine::Key {
                keys: "x",
                text: "Delete it, asking first: keep or delete its branch; its running sessions are stopped first (named before you confirm)",
            },
            HelpLine::Key {
                keys: "x",
                text: "On the \"Merged · safe to delete\" heading: delete all of them and their branches, one confirmation",
            },
            HelpLine::Key {
                keys: "Enter  o",
                text: "Select its session in the sidebar",
            },
            HelpLine::Key {
                keys: "M  E",
                text: "Mark it merged or clear the marker / open it in your editor",
            },
            HelpLine::Key {
                keys: "R  j k  g G",
                text: "Check them all again / move",
            },
            HelpLine::Note {
                note: "Same safety as x on a session: uncommitted work or commits only on the branch need DELETE typed; the main checkout and the one this Deckhand runs in are never deleted. Its sessions are archived afterwards (f A shows them).",
            },
        ],
    },
    HelpTopic {
        title: "Git & PRs",
        lines: &[
            HelpLine::Key {
                keys: "i",
                text: "Workspace overview: branch, changes, diff size, base, upstream",
            },
            HelpLine::Key {
                keys: "m",
                text: "Merge / squash into this branch, uncommitted (Space: commit first; conflicts: Enter keeps, x aborts)",
            },
            HelpLine::Key {
                keys: "→  Enter",
                text: "On the Git tab (the worktree's changes, VS Code style): browse them / open lazygit",
            },
            HelpLine::Heading {
                heading: "Browsing changes (→; ← or Esc back; o opens lazygit)",
            },
            HelpLine::Key {
                keys: "j k  ↑↓",
                text: "Select a file (g/G or Home/End: first/last)",
            },
            HelpLine::Key {
                keys: "Space",
                text: "Stage it, or unstage it if staged (a staged conflict is resolved)",
            },
            HelpLine::Key {
                keys: "a  A",
                text: "Stage everything except conflicts / unstage everything",
            },
            HelpLine::Key {
                keys: "Enter  E",
                text: "Open it in Cursor / VS Code at its first change",
            },
            HelpLine::Key {
                keys: "J K",
                text: "Scroll its diff (Page Up/Down: a page)",
            },
            HelpLine::Heading {
                heading: "Inside i (Esc closes; ↑↓, j k and PgUp/PgDn move through it)",
            },
            HelpLine::Key {
                keys: "c",
                text: "Create a PR: push the branch, then open GitHub's PR form (asks first)",
            },
            HelpLine::Key {
                keys: "P",
                text: "Fetch PR and check status (gh)",
            },
            HelpLine::Key {
                keys: "b",
                text: "Open the PR in the browser",
            },
            HelpLine::Key {
                keys: "g",
                text: "Open the Git tab",
            },
        ],
    },
    HelpTopic {
        title: "Dev & actions",
        lines: &[
            HelpLine::Key {
                keys: "r",
                text: "Run: Dev first (start or stop it), then your actions; Enter runs the selected one",
            },
            HelpLine::Key {
                keys: "x",
                text: "In the run list: stop the running action (or Dev)",
            },
            HelpLine::Key {
                keys: "d  d d",
                text: "The Dev tab (Enter opens it while it runs) / twice quickly: start or stop Dev",
            },
            HelpLine::Note {
                note: "Dev is shared by every session in the same worktree and keeps running after their agents exit, until stopped (or the worktree is deleted).",
            },
            HelpLine::Note {
                note: "An action runs in its own process beside the shell and Dev, one at a time per worktree, and shows on the Terminal tab (v: back to the shell); Enter opens it while it runs; its output and exit code stay until the next one.",
            },
            HelpLine::Note {
                note: "Set the Dev command, actions and a setup command for new worktrees in Settings (C).",
            },
        ],
    },
    HelpTopic {
        title: "Settings",
        lines: &[
            HelpLine::Note {
                note: "Two columns: Global (every repo) and This repo (deckhand.json in the main checkout; fine to commit). This repo wins.",
            },
            HelpLine::Key {
                keys: "● ⚠",
                text: "In effect / a repo value that applies once trusted ((built-in): Deckhand's default)",
            },
            HelpLine::Key {
                keys: "j k  ← →",
                text: "Setting / column (Tab too; PgUp/PgDn, Home/End jump)",
            },
            HelpLine::Key {
                keys: "Enter  x",
                text: "Edit / clear the cell (Actions: a adds, x removes; Linked items: Space, a adds a path, Enter saves)",
            },
            HelpLine::Key {
                keys: "E  T",
                text: "The column's raw JSON (Ctrl+S saves, Ctrl+F formats) / review and trust the repo file",
            },
            HelpLine::Note {
                note: "A repo's commands only run once you trust it; you're asked right before. Your edits here keep it trusted.",
            },
            HelpLine::Heading {
                heading: "Agent signals (Settings → Agents; on for Claude unless switched off)",
            },
            HelpLine::Note {
                note: "Sessions report working, needs input, done, failed or rate-limited (! jumps to them) and what they wait on (a question, a plan, a command, their last line), and can notify you. Claude needs nothing; Codex: switch on, then `deckhand hooks codex` in ~/.codex/hooks.json; Pi has none. \"Done\" means it stopped, not that it succeeded.",
            },
        ],
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn help_topics_match_help_ts() {
        let expected: serde_json::Value =
            serde_json::from_str(include_str!("../tests/data/help_topics.json")).expect("fixture parses");
        assert_eq!(serde_json::to_value(HELP_TOPICS).expect("serialises"), expected);
    }

    #[test]
    fn help_topics_parity() {
        deckhand_parity::replay(
            "P12",
            "help/HELP_TOPICS",
            |_case: &deckhand_parity::Case| -> Result<&[HelpTopic], String> { Ok(HELP_TOPICS) },
        );
    }

    // covers: B-P12-01 help: the Sidebar topic explains every row glyph, the gutter marker, the header and the details
    #[test]
    fn sidebar_topic_explains_every_glyph() {
        let topic = HELP_TOPICS
            .iter()
            .find(|entry| entry.title == "Sidebar")
            .expect("a Sidebar topic");
        let text = topic
            .lines
            .iter()
            .map(|line| match line {
                HelpLine::Key { keys, text } => format!("{keys} {text}"),
                HelpLine::Note { note } => (*note).to_owned(),
                HelpLine::Heading { heading } => (*heading).to_owned(),
            })
            .collect::<Vec<_>>()
            .join("\n");
        for glyph in [
            "⠋",
            "●",
            "◌",
            "○",
            "!",
            "?",
            "◆",
            "⌛",
            "▾",
            "▸",
            "↳",
            "⑂",
            "▶",
            "▣",
            "✓",
            "+N",
            "✶",
            "π",
            "◇",
            "↑",
            "╎",
            "! N",
            "Below the list",
        ] {
            assert!(text.contains(glyph), "{glyph}");
        }
    }
}
