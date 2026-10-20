//! Port of tests/tasksUi.test.ts: the Tasks board in a real PTY, driving the isolated dev launcher.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use deckhand_e2e::harness::{Sandbox, launcher, wait_for};
use deckhand_e2e::ui::{Ui, UiOptions};

const UI_WAIT: Duration = Duration::from_secs(30);

/// A stand-in agent (never a real one): stays alive and prints a marker. Installs claude, pi and codex in `dir`.
fn install_fake_agents(dir: &Path) {
    fs::create_dir_all(dir).expect("agent bin dir");
    let script = "#!/bin/sh\n\
        for arg in \"$@\"; do\n\
        \tcase \"$arg\" in\n\
        \t--help) echo 'PROMPT --settings --no-daemon resume'; exit 0;;\n\
        \t--version) echo '1.0.0'; exit 0;;\n\
        \tesac\n\
        done\n\
        echo 'fake agent ready'\n\
        exec sleep 100000\n";
    for agent in ["claude", "pi", "codex"] {
        let file = dir.join(agent);
        fs::write(&file, script).expect("write agent");
        fs::set_permissions(&file, fs::Permissions::from_mode(0o755)).expect("chmod agent");
    }
}

/// The dev launcher in a PTY with the fake agents first on PATH.
fn launch_with_agents(sandbox: &Sandbox) -> Ui {
    let home = sandbox.dev_home();
    let bin = home.join("bin");
    install_fake_agents(&bin);
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap_or_default());
    let script = launcher().display().to_string();
    Ui::spawn(
        sandbox,
        UiOptions {
            program: Path::new("/bin/sh"),
            args: &[script.as_str(), "--sandbox"],
            cwd: &home,
            env: &[("PATH", path.as_str())],
            dev: true,
        },
    )
}

/// The only `.md` file in `directory` (empty text unless there is exactly one).
fn only_note(directory: PathBuf) -> String {
    let names: Vec<_> = fs::read_dir(directory)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .filter(|path| path.extension().is_some_and(|ext| ext == "md"))
                .collect()
        })
        .unwrap_or_default();
    match names.as_slice() {
        [one] => fs::read_to_string(one).unwrap_or_default(),
        _ => String::new(),
    }
}

fn is_hex8(text: &str) -> bool {
    text.len() == 8 && text.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Every text following an occurrence of `prefix` (the part of the file after each match).
fn after<'a>(text: &'a str, prefix: &str) -> impl Iterator<Item = &'a str> {
    let prefix = prefix.to_string();
    text.match_indices(prefix.clone().as_str())
        .map(|(at, _)| &text[at + prefix.len()..])
        .collect::<Vec<_>>()
        .into_iter()
}

/// `text` contains `prefix` + 8 lowercase hex digits + `suffix`.
fn has_hex8(text: &str, prefix: &str, suffix: &str) -> bool {
    after(text, prefix).any(|rest| rest.len() >= 8 && is_hex8(&rest[..8]) && rest[8..].starts_with(suffix))
}

/// `^- [ ] Write the docs <!-- dh:t=[0-9a-f]{8} added=\d{4}-\d\d-\d\d -->\n  why: people ask\n$`
fn is_first_task(text: &str) -> bool {
    let Some(rest) = text.strip_prefix("- [ ] Write the docs <!-- dh:t=") else {
        return false;
    };
    if rest.len() < 8 || !is_hex8(&rest[..8]) {
        return false;
    }
    let Some(rest) = rest[8..].strip_prefix(" added=") else {
        return false;
    };
    let Some(date) = rest.get(..10) else {
        return false;
    };
    let digits = |part: &str, n: usize| part.len() == n && part.bytes().all(|b| b.is_ascii_digit());
    let parts: Vec<&str> = date.split('-').collect();
    parts.len() == 3
        && digits(parts[0], 4)
        && digits(parts[1], 2)
        && digits(parts[2], 2)
        && &rest[10..] == " -->\n  why: people ask\n"
}

/// `/ wt=[0-9a-f-]{36}/`
fn has_worktree_id(text: &str) -> bool {
    after(text, " wt=").any(|rest| {
        rest.get(..36).is_some_and(|id| {
            id.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b) || b == b'-')
        })
    })
}

/// `/Write the docs +… │/`: a row that does not fit its pane.
fn has_truncated_row(screen: &str) -> bool {
    after(screen, "Write the docs").any(|rest| {
        let trimmed = rest.trim_start_matches(' ');
        trimmed.len() < rest.len() && trimmed.starts_with("… │")
    })
}

// ported from tests/tasksUi.test.ts: Tasks board: b opens it, a adds a task, n starts a session from it (base branch in the form), the Notes tab shows it, ctrl+p sends a note item to it
#[test]
fn tasks_board_add_start_session_notes_and_send() {
    let sandbox = Sandbox::new("ui");
    let home = sandbox.dev_home();
    let mut ui = launch_with_agents(&sandbox);
    let tasks_file = || only_note(home.join("notes/tasks"));
    ui.screen("DEV (isolated)");
    ui.screen("● ready");

    ui.press("b");
    ui.screen("☐ Tasks");
    ui.screen("No tasks yet · a adds one");
    ui.screen("a add");
    ui.press("a");
    ui.screen("New task");
    ui.screen("tab details");
    ui.press("Write the docs");
    ui.screen("Write the docs");
    ui.press("\t");
    ui.screen("ctrl+s save");
    // Each control key waits for the screen: a key written with the text before it can arrive as one chunk.
    ui.press("why: people ask");
    ui.screen("why: people ask");
    ui.press("\x13");
    ui.screen("Added to the backlog");
    ui.screen("☐ 1 task · b");
    assert!(is_first_task(&tasks_file()), "tasks file: {:?}", tasks_file());

    // n: the usual form, named after the task, in a new worktree; ↑↓ picks where its branch starts.
    ui.press("n");
    ui.screen("Choose an agent");
    ui.press("\r");
    ui.screen("Task: Write the docs");
    ui.screen("Name: Write the docs");
    ui.screen("Workspace: new worktree");
    ui.screen("Base: current checkout");
    ui.screen("↑↓ 1/");
    ui.press("\x1b[B");
    ui.screen("↑↓ 2/");
    ui.press("\x1b[A");
    ui.screen("↑↓ 1/");
    ui.press("\r");
    ui.screen("fake agent ready");
    let linked = wait_for(&tasks_file, |text| has_worktree_id(text), UI_WAIT);
    assert!(
        has_hex8(&linked, "- [ ] Write the docs <!-- dh:t=", " wt="),
        "linked: {linked:?}"
    );

    ui.press("a");
    ui.screen("◆ Task Write the docs");
    ui.screen("b board");
    // The board groups open tasks by worktree: ◆ its task. v shows only this session's worktree, where a (the work
    // has its task) adds a step: an item of its note, listed under the task.
    ui.press("b");
    ui.screen("◆ Write the docs");
    ui.screen("⎇ write");
    ui.screen("o session");
    assert!(
        !has_truncated_row(&ui.screen("◆ Write the docs")),
        "a row fits its pane"
    );
    ui.press("v");
    ui.screen("v all tasks");
    ui.press("a");
    ui.screen("New step · the note of ⎇ write");
    ui.press("Add tests");
    ui.screen("Add tests");
    ui.press("\r");
    ui.screen("Added a step to ⎇ write");
    ui.screen("☐ Add tests");
    ui.screen("0/1");
    assert!(!tasks_file().contains("Add tests"));
    ui.press("v");
    ui.screen("BACKLOG");
    ui.press("\x1b");
    ui.screen("0/1 steps");
    ui.press("\x1b");
    ui.screen("enter edit notes");

    // A note's checklist item goes to Tasks with ctrl+p and leaves a ↗ link in the note.
    ui.press("\r");
    ui.screen("esc done");
    ui.press("\x14");
    ui.screen("- [ ]");
    ui.press("Follow up on review");
    ui.screen("ctrl+p → tasks");
    ui.press("\x10");
    ui.screen("Sent to Tasks");
    ui.press("\x1b");
    ui.screen("↗ Follow up on review · in Tasks");
    // A note item sent to Tasks is work of its own: the backlog.
    ui.press("b");
    ui.screen("Follow up on review");
    ui.screen("BACKLOG · 1");
    assert!(
        has_hex8(&tasks_file(), "- [ ] Follow up on review <!-- dh:t=", " added="),
        "tasks file: {:?}",
        tasks_file()
    );
    // tab: every note, by worktree; f: only open checklist items (the step, not the ↗ line).
    ui.press("\t");
    ui.screen("✎ Notes");
    ui.screen("Worktree note");
    ui.screen("↗ Follow up on review · in Tasks");
    ui.press("f");
    ui.screen("f every note");
    ui.screen("☐ Add tests");
    ui.press("f");
    ui.screen("↗ Follow up on review · in Tasks");
    ui.press("\t");
    ui.screen("☐ Tasks");
    // w: the backlog and the way back to its note come first, then the worktrees (Tab: main-checkout sessions); j/k move,
    // / searches (letters are text only then). ⎇ write has its task, so there it would become a step.
    ui.press("G");
    ui.screen("› ○ Follow up on review");
    ui.press("w");
    ui.screen("Move “Follow up on review” to");
    ui.screen("↩ Back to its note");
    ui.screen("› ⎇ write");
    ui.screen("→ a step in its note");
    ui.press("\t");
    ui.screen("No sessions in the main checkout");
    ui.press("\t");
    ui.screen("› ⎇ write");
    ui.press("k");
    ui.screen("› ↩ Back to its note");
    ui.press("j");
    ui.screen("› ⎇ write");
    ui.press("/");
    ui.screen("esc clear");
    ui.press("zz");
    ui.screen("Nothing in worktrees matches “zz”");
    ui.press("\x1b");
    ui.screen("/ search");
    // Enter keeps the search and gives j/k back; Esc then clears it.
    ui.press("/");
    ui.screen("type to search");
    ui.press("wr");
    ui.screen("search: wr▏");
    ui.press("\r");
    ui.screen("/ edit search");
    ui.screen("› ⎇ write");
    ui.press("\x1b");
    ui.screen("/ search");
    ui.press("k");
    ui.screen("› ↩ Back to its note");
    ui.press("\x1b[A");
    ui.screen("› Backlog · no worktree");
    ui.press("\r");
    ui.screen("It is already there");
    // Back to the note it was sent from: an open item there again, gone from the list.
    ui.press("w");
    ui.screen("↩ Back to its note");
    ui.press("\x1b[A");
    ui.screen("› ↩ Back to its note");
    ui.press("\r");
    ui.screen("Back in the note of");
    ui.screen("Nothing waiting");
    assert!(!tasks_file().contains("Follow up on review"));
    // The only task is on a worktree, where a adds a step; the empty backlog can be selected to add a task there.
    ui.press("g");
    ui.screen("a step •");
    // A adds a backlog task from there too.
    ui.press("A");
    ui.screen("New task · backlog");
    ui.press("\x1b");
    ui.screen("a step •");
    ui.press("G");
    ui.screen("› Nothing waiting");
    ui.screen("a add task");
    ui.press("a");
    ui.screen("New task · backlog");
    ui.press("\x1b");
    ui.screen("a add task");
    // o jumps to the task's session even while the sidebar's filter hides it: the filter goes back to active, so the
    // session is selected (not the first listed one, or none).
    ui.press("\x1b");
    ui.screen("enter edit notes");
    ui.press("f");
    ui.press("e");
    ui.screen("No sessions match");
    // One key per wait (a g read before the board is open goes to the session list), and '• o session' (the hint)
    // because the sidebar beside the board says "No sessions match", which contains "o session".
    ui.press("b");
    ui.screen("☐ Tasks · sandbox");
    ui.press("g");
    ui.screen("• o session");
    ui.press("o");
    ui.screen("enter edit notes");

    // Keymap: [ goes to the previous tab (Notes → Dev), x asks before stopping, Esc backs out of it, Space marks done.
    ui.press("\x1b");
    ui.screen("enter edit notes");
    ui.press("[");
    ui.screen("d d start Dev");
    ui.press("x");
    ui.screen("Kill only, keep worktree");
    ui.press("\x1b");
    ui.screen("d d start Dev");
    ui.press("\x1b[A");
    ui.press("\x1b[B");
    ui.screen("x stop");
    ui.press(" ");
    ui.screen("Marked done");
    // Backspace archives a finished session; in the archived view (f A) Backspace removes it for good.
    ui.press("X");
    ui.screen("s resume");
    ui.press("x");
    ui.screen("Already stopped: Backspace archives it, s resumes it");
    ui.press("\x7f");
    ui.screen("Archived Write the docs");
    ui.screen("No sessions match");
    ui.press("f");
    ui.screen("A  archived");
    ui.press("A");
    ui.screen("A unarchive");
    ui.screen("backspace remove");
    // Removing always asks; its note has the open item sent back from Tasks, so it is listed and x removes anyway.
    ui.press("\x7f");
    ui.screen("for good?");
    ui.screen("Follow up on review");
    ui.press("x");
    ui.screen("No sessions yet.");
}
