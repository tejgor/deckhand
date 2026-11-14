//! The Notes tab in a real PTY: the session's line and steps, its line's note and its own note.

use std::fs;
use std::path::Path;

use deckhand_e2e::fakes::install_sandbox_agents;
use deckhand_e2e::harness::{Sandbox, wait_for};
use deckhand_e2e::ui::{UI_WAIT, launch_sandbox_ui};

/// The only `.md` file of `notes/<kind>` (none unless there is exactly one), and its path.
fn notes_file(home: &Path, kind: &str) -> Option<(std::path::PathBuf, String)> {
    let directory = home.join("notes").join(kind);
    let paths: Vec<_> = fs::read_dir(&directory)
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| path.extension().is_some_and(|ext| ext == "md"))
                .collect()
        })
        .unwrap_or_default();
    match paths.as_slice() {
        [only] => fs::read_to_string(only).ok().map(|text| (only.clone(), text)),
        _ => None,
    }
}

fn text_of(home: &Path, kind: &str) -> Option<String> {
    notes_file(home, kind).map(|(_, text)| text)
}

// The session's own note: edited with a real cursor (arrows, word jumps), ctrl+x makes a checklist item, a paste is
// text, esc shows it rendered; the file and an editor's edit round-trip. Its line's note, written for a session on no
// line, makes its line; a adds a step there and space ticks it; ctrl+p makes a checkbox of its note a step.
#[test]
fn notes_tab_edits_both_notes_and_the_steps_and_round_trips_files() {
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

    // A session started from the sidebar is on no line yet; Enter edits its own note first.
    ui.press("a");
    ui.screen("No line yet: writing here makes one");
    ui.screen("No notes for this session · enter to write");
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
    // A bracketed paste is text: its newline and Tab are inserted (at the end of the line: Ctrl+E).
    ui.press("\x05");
    ui.press("\x1b[200~\r\tpasted\x1b[201~");
    ui.screen(" pasted");
    ui.press("\x1b");
    ui.screen("enter edit notes");
    // Read mode renders the item.
    ui.screen("☐ so hello big world");
    wait_for(
        || text_of(&home, "sessions"),
        |text| text.as_deref() == Some("- [ ] so hello big world\n\tpasted"),
        UI_WAIT,
    );
    // An edit made in an editor shows up without a restart.
    let (path, _) = notes_file(&home, "sessions").expect("the session's note");
    fs::write(path, "shared plan\n- [ ] ship it").expect("edit note");
    ui.screen("☐ ship it");
    assert_eq!(text_of(&home, "lines"), None, "no line note yet");

    // Shift+Tab: its line's note. Writing there makes the session's line, named after it.
    ui.press("\r");
    ui.screen("esc done");
    ui.screen("No line yet: writing here makes one · tab to write");
    ui.press("\x1b[Z");
    ui.screen("tab session's note");
    ui.press("why: flaky on CI");
    ui.screen("◆ nts");
    ui.screen("nts · shared with whoever works on it");
    wait_for(
        || text_of(&home, "lines"),
        |text| text.as_deref() == Some("why: flaky on CI"),
        UI_WAIT,
    );
    let list = wait_for(
        || text_of(&home, "tasks"),
        |text| {
            text.as_deref()
                .is_some_and(|text| text.starts_with("- [ ] nts <!-- dh:t="))
        },
        UI_WAIT,
    )
    .unwrap_or_default();
    assert!(list.contains(" s="), "the session is on it: {list}");

    // Shift+Tab again: the steps. a adds one under the line; space ticks it, and the line counts it.
    ui.press("\x1b[Z");
    ui.screen("a add a step");
    ui.press("a");
    ui.screen("enter save");
    ui.press("Reproduce it");
    ui.screen("› ☐ Reproduce it");
    ui.press("\r");
    ui.screen("Added a step: Reproduce it");
    ui.screen("◆ nts · 0/1 steps");
    ui.screen("› ☐ Reproduce it");
    ui.press(" ");
    ui.screen("Done: Reproduce it");
    ui.screen("◆ nts · 1/1 steps");
    wait_for(
        || text_of(&home, "tasks"),
        |text| {
            text.as_deref()
                .is_some_and(|text| text.contains("\n  - [x] Reproduce it <!-- dh:t="))
        },
        UI_WAIT,
    );

    // Tab twice: its own note. Ctrl+P on its open checkbox (the cursor is at the end) makes it a step.
    ui.press("\t");
    ui.screen("tab session's note");
    ui.press("\t");
    ui.screen("ctrl+p → step");
    ui.press("\x10");
    ui.screen("Now a step of this session's line: ship it");
    ui.screen("◆ nts · 1/2 steps");
    wait_for(
        || text_of(&home, "sessions"),
        |text| text.as_deref() == Some("shared plan"),
        UI_WAIT,
    );
    wait_for(
        || text_of(&home, "tasks"),
        |text| {
            text.as_deref()
                .is_some_and(|text| text.contains("\n  - [ ] ship it <!-- dh:t="))
        },
        UI_WAIT,
    );
    ui.press("\x1b");
    ui.screen("enter edit notes");
    // The tab counts the open step.
    ui.screen("a:Notes ☐ 1");
}
