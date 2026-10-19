//! Smoke test for the PTY harness itself (`deckhand_e2e::ui`), independent of the binary under test.

use std::path::Path;
use std::time::Duration;

use deckhand_e2e::harness::Sandbox;
use deckhand_e2e::ui::{Ui, UiOptions};

#[test]
fn pty_ui_screen_press_and_exit() {
    let sandbox = Sandbox::new("ui-smoke");
    let sh = Path::new("/bin/sh");
    let mut ui = Ui::spawn(
        &sandbox,
        UiOptions {
            program: sh,
            args: &[
                "-c",
                "printf 'cols=%s term=%s\\n' \"$(tput cols)\" \"$TERM\"; read line; printf 'got:%s\\n' \"$line\"",
            ],
            cwd: sandbox.root(),
            env: &[],
            dev: false,
        },
    );
    // The screen is a model: the cursor-addressed text is found as laid out.
    let screen = ui.screen("cols=130 term=xterm-256color");
    assert!(screen.split('\n').count() == 36);
    ui.press("hello\r");
    ui.screen("got:hello");
    let status = ui.ended(Duration::from_secs(5)).expect("sh exits");
    assert!(status.success());
}
