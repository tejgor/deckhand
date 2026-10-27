//! The ? guide as data: short topics of key → what it does, with a few notes. Rendered by the help pane (topic list,
//! aligned key column, search). The UI keymap tests check that documented shortcuts match their bindings.

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
                keys: "C  G",
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
                keys: "U",
                text: "Mark its finished reply unread (◆) to come back to it / read again",
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
                text: "Dev runs in this worktree (first row; muted below)",
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
                keys: "< > =",
                text: "Narrow / widen the sidebar (saved) / fit it to the sessions again",
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
                text: "Step into the pane: type to the agent (running), browse Git changes, edit the notes",
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
                keys: "v",
                text: "On Dev with several processes: select the next one (Enter opens it)",
            },
            HelpLine::Key {
                keys: "E",
                text: "Open in Cursor / VS Code: the note on Notes, otherwise the session's folder",
            },
            HelpLine::Heading {
                heading: "Typing to the agent (→ on Preview)",
            },
            HelpLine::Key {
                keys: "Ctrl+]",
                text: "Back (Ctrl+Space too); every other key goes to the agent, Esc and Ctrl+C included",
            },
            HelpLine::Note {
                note: "Mouse wheel: Scroll speed in Settings (C); \"claude_scroll_override\": false in config.json keeps your own Claude scroll settings.",
            },
            HelpLine::Note {
                note: "Agents ask for approvals in their own terminal: answer them here (→) or open it (Enter). Terminal, Git and Dev belong to the worktree: all its sessions share them, also after their agents exit.",
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
                keys: "o  J K  c",
                text: "Open the session doing it / reorder (j k, g G move) / hide or show its steps (▸ 2/5)",
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
            HelpLine::Key {
                keys: "J K",
                text: "In a deletion confirmation that does not fit: scroll it (PgUp/PgDn too)",
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
                text: "On the Git tab (the worktree's changes, then what its branch committed): browse them / open lazygit",
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
                heading: "Inside i (Esc closes; ↑↓ and j k move through it, J/K or PgUp/PgDn a page)",
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
                keys: "r",
                text: "In the run list, with several Dev processes: a row for each after Dev; Enter starts or restarts it, x stops it",
            },
            HelpLine::Key {
                keys: "d  d d",
                text: "The Dev tab (Enter opens it while it runs) / twice quickly: start or stop Dev",
            },
            HelpLine::Note {
                note: "Dev is shared by every session in the same worktree and keeps running after their agents exit, until stopped (or the worktree is deleted).",
            },
            HelpLine::Note {
                note: "Dev can run several processes side by side, such as a backend and a frontend: in Settings (C), Enter on the Dev command lists them and + Add a process adds one. The Dev tab splits to show each one's output.",
            },
            HelpLine::Note {
                note: "Dev runs in one worktree of a repository at a time, since they share its ports: starting it in one worktree stops it in the others.",
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
                text: "Setting / column (Tab too; J/K, g/G jump)",
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

    fn topic(title: &str) -> &'static HelpTopic {
        HELP_TOPICS
            .iter()
            .find(|entry| entry.title == title)
            .expect("guide topic")
    }

    fn topic_text(title: &str) -> String {
        topic(title)
            .lines
            .iter()
            .map(|line| match line {
                HelpLine::Key { keys, text } => format!("{keys} {text}"),
                HelpLine::Note { note } => (*note).to_owned(),
                HelpLine::Heading { heading } => (*heading).to_owned(),
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn guide_covers_the_main_workflows_in_topic_order() {
        assert_eq!(
            HELP_TOPICS.iter().map(|entry| entry.title).collect::<Vec<_>>(),
            [
                "Start here",
                "Sessions",
                "Sidebar",
                "Find & organize",
                "Panes",
                "Notes",
                "Tasks",
                "Worktrees",
                "Git & PRs",
                "Dev & actions",
                "Settings",
            ]
        );
        for topic in HELP_TOPICS {
            assert!(!topic.lines.is_empty(), "{} has no guide content", topic.title);
            for line in topic.lines {
                let text = match line {
                    HelpLine::Key { keys, text } => {
                        assert!(!keys.trim().is_empty(), "{} has an unlabeled shortcut", topic.title);
                        text
                    }
                    HelpLine::Note { note } => note,
                    HelpLine::Heading { heading } => heading,
                };
                assert!(!text.trim().is_empty(), "{} has an empty guide line", topic.title);
            }
        }
    }

    #[test]
    fn guide_explains_fitting_the_sidebar_and_the_dev_marker() {
        let fit = topic("Find & organize")
            .lines
            .iter()
            .find_map(|line| match line {
                HelpLine::Key { keys: "< > =", text } => Some(*text),
                _ => None,
            })
            .expect("sidebar resize and fit shortcuts");
        assert!(fit.contains("saved"));
        assert!(fit.contains("fit it to the sessions"));
        assert!(topic_text("Sidebar").contains("Dev runs in this worktree (first row; muted below)"));
    }

    #[test]
    fn guide_explains_multiple_dev_processes_and_worktree_exclusivity() {
        let dev = topic_text("Dev & actions");
        for instruction in [
            "a row for each after Dev",
            "Enter starts or restarts it, x stops it",
            "+ Add a process",
            "splits to show each one's output",
            "starting it in one worktree stops it in the others",
        ] {
            assert!(dev.contains(instruction), "missing Dev instruction: {instruction}");
        }
        assert!(topic("Panes").lines.iter().any(|line| matches!(
            line,
            HelpLine::Key { keys: "v", text } if text.contains("select the next one (Enter opens it)")
        )));
    }

    #[test]
    fn guide_explains_typing_to_the_agent() {
        let panes = topic_text("Panes");
        assert!(panes.contains("type to the agent (running)"));
        assert!(panes.contains("Ctrl+] Back (Ctrl+Space too)"));
        assert!(panes.contains("Esc and Ctrl+C included"));
    }

    #[test]
    fn guide_explains_scroll_speed_and_preserving_claude_scroll_settings() {
        let panes = topic_text("Panes");
        assert!(
            !panes.contains("+ − change wheel speed"),
            "the wheel speed is set in Settings only"
        );
        assert!(panes.contains("Scroll speed in Settings (C)"));
        assert!(panes.contains("\"claude_scroll_override\": false"));
    }

    #[test]
    fn sidebar_topic_explains_every_glyph() {
        let text = topic_text("Sidebar");
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
