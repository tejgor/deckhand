//! Port of tests/notesUi.test.ts: the Notes tab in a real PTY.

use std::fs;
use std::path::Path;

use deckhand_e2e::fakes::install_sandbox_agents;
use deckhand_e2e::harness::{Sandbox, wait_for};
use deckhand_e2e::ui::{UI_WAIT, launch_sandbox_ui};

fn notes_file(home: &Path, kind: &str) -> Option<String> {
    let directory = home.join("notes").join(kind);
    let names: Vec<String> = fs::read_dir(&directory)
        .map(|entries| {
            entries
                .flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|name| name.ends_with(".md"))
                .collect()
        })
        .unwrap_or_default();
    match names.as_slice() {
        [only] => fs::read_to_string(directory.join(only)).ok(),
        _ => None,
    }
}

// ported from tests/notesUi.test.ts: Notes tab: o edits with a real cursor (arrows, word jumps), ctrl+x makes a checklist item, tab edits the worktree's note, esc shows them rendered; files and editor edits round-trip
#[test]
fn notes_tab_edits_with_a_real_cursor_and_round_trips_files() {
    let sandbox = Sandbox::new("nts");
    let bin = install_sandbox_agents(&sandbox, "echo 'fake agent ready'");
    let mut ui = launch_sandbox_ui(&sandbox, &bin, &[]);
    let home = sandbox.dev_home();
    ui.screen("DEV (isolated)");
    ui.screen("● ready");
    // The sandbox preselects a new worktree.
    ui.press("n");
    ui.screen("Choose an agent");
    ui.press("\r");
    ui.screen("Workspace: new worktree");
    ui.press("nts");
    ui.screen("Name: nts");
    ui.press("\r");
    ui.screen("fake agent ready");

    // A session in a worktree has one note, the worktree's.
    ui.press("a");
    ui.screen("No worktree notes · enter to add");
    ui.screen("E open in editor");
    ui.press("\r");
    ui.screen("esc done");
    ui.press("hello world");
    ui.screen("hello world");
    // Left x5 lands before "world"; Alt+Left (ESC b, as macOS terminals send Option+Left) jumps a word back.
    ui.press("\x1b[D\x1b[D\x1b[D\x1b[D\x1b[D");
    ui.press("big ");
    ui.screen("hello big world");
    ui.press("\x1bb");
    ui.press("\x1bb");
    ui.press("so ");
    ui.screen("so hello big world");
    // Ctrl+X makes the line a checklist item (raw Markdown while editing).
    ui.press("\x18");
    ui.screen("- [ ] so hello big world");
    // There is no second note to switch to.
    ui.press("\t");
    ui.screen("one note, shared by its sessions");
    // A bracketed paste is text: its newline and Tab are inserted (at the end of the line: Ctrl+E).
    ui.press("\x05");
    ui.screen("so hello big world");
    ui.press("\x1b[200~\r\tpasted\x1b[201~");
    ui.screen(" pasted");
    ui.press("\x1b");
    ui.screen("enter edit notes");
    // Read mode renders the item; the sidebar counts it.
    ui.screen("☐ so hello big world");
    ui.screen("☐ 1 open");
    wait_for(
        || notes_file(&home, "worktrees"),
        |text| text.as_deref() == Some("- [ ] so hello big world\n\tpasted"),
        UI_WAIT,
    );
    // An edit made in an editor shows up without a restart.
    let directory = home.join("notes").join("worktrees");
    let name = fs::read_dir(&directory)
        .expect("notes dir")
        .flatten()
        .map(|e| e.path())
        .find(|p| p.extension().is_some_and(|x| x == "md"))
        .expect("note file");
    fs::write(name, "shared plan\n- [x] edited in VS Code").expect("edit note");
    ui.screen("☑ edited in VS Code");
    assert_eq!(
        notes_file(&home, "sessions"),
        None,
        "nothing was written to a note of its own"
    );
}
