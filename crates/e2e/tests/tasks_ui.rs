//! The Tasks board (the repository's one list, as a tree) in a real PTY, driving the isolated dev launcher.

use std::fs;
use std::path::{Path, PathBuf};

use deckhand_e2e::fakes::{READY, install_sh_agents};
use deckhand_e2e::harness::{Sandbox, wait_for};
use deckhand_e2e::ui::{UI_WAIT, Ui, launch_sandbox_ui};

/// The dev launcher in a PTY with the fake agents (in `<dev home>/bin`) first on PATH.
fn launch_with_agents(sandbox: &Sandbox) -> Ui {
    let bin = install_sh_agents(&sandbox.dev_home().join("bin"), READY);
    launch_sandbox_ui(sandbox, &bin, &[])
}

/// The `.md` files in `directory`.
fn notes_in(directory: &Path) -> Vec<PathBuf> {
    fs::read_dir(directory)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .filter(|path| path.extension().is_some_and(|ext| ext == "md"))
                .collect()
        })
        .unwrap_or_default()
}

/// The only `.md` file in `directory` (empty text unless there is exactly one).
fn only_note(directory: &Path) -> String {
    match notes_in(directory).as_slice() {
        [one] => fs::read_to_string(one).unwrap_or_default(),
        _ => String::new(),
    }
}

fn is_hex8(text: &str) -> bool {
    text.len() == 8 && text.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// The ID of the line titled `title` in the list's text (`- [ ] <title> <!-- dh:t=<id> ...`).
fn line_id(text: &str, title: &str) -> Option<String> {
    let prefix = format!("{title} <!-- dh:t=");
    let at = text.find(&prefix)? + prefix.len();
    let id = text.get(at..at + 8)?;
    is_hex8(id).then(|| id.to_owned())
}

/// The bookkeeping of the line titled `title` (what follows `dh:` up to `-->`).
fn meta_of(text: &str, title: &str) -> String {
    let prefix = format!("{title} <!-- dh:");
    text.find(&prefix)
        .and_then(|at| {
            let rest = &text[at + prefix.len()..];
            rest.find(" -->").map(|end| rest[..end].to_owned())
        })
        .unwrap_or_default()
}

// The board is the list: a adds a line (and its note), n starts a session on it, a nested line gets a sub-session of
// that session, the Notes view shows the line notes and the scratchpad (ctrl+p there makes a checkbox a line), w moves a
// line under another, x removes one with its lines.
#[test]
fn tasks_board_is_the_list_with_sessions_on_lines_and_notes() {
    let sandbox = Sandbox::new("ui");
    let home = sandbox.dev_home();
    let mut ui = launch_with_agents(&sandbox);
    let list = || only_note(&home.join("notes/tasks"));
    ui.screen("DEV (isolated)");
    ui.screen("● ready");

    // a: a top-level line with a note to start it with.
    ui.press("b");
    ui.screen("☐ Tasks");
    ui.screen("No lines yet · a adds one");
    ui.screen("a add line");
    ui.press("a");
    ui.screen("New line · top level (the backlog)");
    ui.screen("tab note");
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
    let text = wait_for(&list, |text| line_id(text, "Write the docs").is_some(), UI_WAIT);
    let docs = line_id(&text, "Write the docs").unwrap_or_default();
    assert!(text.starts_with("- [ ] Write the docs <!-- dh:t="), "list: {text:?}");
    let note = wait_for(
        || fs::read_to_string(home.join(format!("notes/lines/{docs}.md"))).unwrap_or_default(),
        |text| text.contains("why: people ask"),
        UI_WAIT,
    );
    assert_eq!(note.trim_end(), "why: people ask");
    // The details show the note.
    ui.screen("Nobody is on it · n starts a session on it");

    // n: the usual form, named after the line, in a new worktree; the session goes on the line.
    ui.press("n");
    ui.screen("Choose an agent");
    ui.press("\r");
    ui.screen("Task: Write the docs");
    ui.screen("Name: Write the docs");
    ui.screen("Workspace: new worktree");
    ui.screen("Base: current checkout");
    ui.press("\r");
    ui.screen("fake agent ready");
    let on = wait_for(&list, |text| meta_of(text, "Write the docs").contains(" s="), UI_WAIT);
    assert!(
        meta_of(&on, "Write the docs").starts_with(&format!("t={docs} s=")),
        "list: {on:?}"
    );

    // b from the session opens on its line: who is on it, and a adds a step under it.
    ui.press("b");
    ui.screen("› ☐ Write the docs");
    ui.screen("is on it");
    ui.screen("o session");
    ui.press("a");
    ui.screen("New line · under “Write the docs”");
    ui.press("Add tests");
    ui.screen("Add tests");
    ui.press("\r");
    ui.screen("Added under “Write the docs”");
    ui.screen("  ☐ Add tests");
    ui.screen("0/1");
    wait_for(&list, |text| text.contains("\n  - [ ] Add tests <!-- dh:t="), UI_WAIT);

    // n on that step: a sub-session of the session on the line above, stacked on its branch.
    ui.screen("› ");
    ui.screen("n start");
    ui.press("n");
    ui.screen("Choose an agent");
    ui.press("\r");
    ui.screen("Task: Add tests");
    ui.screen("· stacked");
    ui.press("\r");
    ui.screen("fake agent ready");
    let stacked = wait_for(&list, |text| meta_of(text, "Add tests").contains(" s="), UI_WAIT);
    let state: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(home.join("state.json")).unwrap_or_default()).unwrap_or_default();
    let sessions = state["sessions"].as_array().cloned().unwrap_or_default();
    let session_of = |title: &str| {
        let s = meta_of(&stacked, title)
            .split(' ')
            .find_map(|part| part.strip_prefix("s=").map(str::to_owned))
            .unwrap_or_default();
        sessions.iter().find(|session| session["id"] == s.as_str()).cloned()
    };
    let parent = session_of("Write the docs").unwrap_or_else(|| panic!("the line's session in {state}"));
    let child = session_of("Add tests").unwrap_or_else(|| panic!("the step's session in {state}"));
    assert_eq!(child["parentSessionId"], parent["id"], "{child}");

    // tab: the Notes view: the scratchpad, then the line notes under their lines.
    ui.press("b");
    ui.screen("☐ Tasks");
    ui.press("\t");
    ui.screen("✎ Notes");
    ui.screen("SCRATCHPAD");
    ui.screen("LINE NOTES");
    ui.screen("why: people ask");
    // f: only notes with text (the empty scratchpad goes).
    ui.press("f");
    ui.screen("f every note");
    ui.press("f");
    ui.screen("f only with text");
    // enter on the scratchpad: the board's editor; ctrl+p on a checkbox makes it a backlog line.
    ui.press("g");
    ui.screen("› Scratchpad");
    ui.press("\r");
    ui.screen("ctrl+p");
    ui.press("- [ ] Follow up on review");
    ui.screen("- [ ] Follow up on review");
    ui.press("\x10");
    ui.screen("Now a line in the backlog: Follow up on review");
    ui.press("\x1b");
    ui.screen("enter edit");
    wait_for(
        &list,
        |text| text.contains("- [ ] Follow up on review <!-- dh:t="),
        UI_WAIT,
    );
    let scratchpad = only_note(&home.join("notes/repos"));
    assert!(
        !scratchpad.contains("Follow up on review"),
        "scratchpad: {scratchpad:?}"
    );
    ui.press("\t");
    ui.screen("☐ Tasks");
    ui.screen("☐ Follow up on review");

    // w: under another line (Lines; / searches), the menu says what each row would make of it.
    ui.press("G");
    ui.screen("› ☐ Follow up on review");
    ui.press("w");
    ui.screen("Move “Follow up on review” to");
    ui.screen("Backlog · top level, nobody on it");
    ui.screen("here");
    ui.press("\t");
    ui.screen("Sessions 2");
    ui.screen("→ a step of its line");
    ui.press("\t");
    ui.screen("Lines 2");
    ui.press("/");
    ui.screen("type to search");
    ui.press("docs");
    ui.screen("search: docs▏");
    ui.press("\r");
    ui.screen("› Write the docs");
    ui.screen("→ under it");
    ui.press("\r");
    ui.screen("Now under “Write the docs”: Follow up on review");
    wait_for(
        &list,
        |text| text.contains("\n  - [ ] Follow up on review <!-- dh:t="),
        UI_WAIT,
    );

    // x twice removes a line with the lines under it.
    ui.press("g");
    ui.screen("› ☐ Write the docs");
    ui.press("j");
    ui.screen("›   ☐ Add tests");
    ui.press("x");
    ui.screen("x again removes “Add tests”");
    ui.press("x");
    ui.screen("Removed Add tests");
    wait_for(&list, |text| !text.contains("Add tests"), UI_WAIT);

    // o selects the session on a line.
    ui.press("g");
    ui.screen("› ☐ Write the docs");
    ui.press("o");
    ui.screen("› 1 ▾ ● Write the docs");
    ui.screen("x stop");
}
