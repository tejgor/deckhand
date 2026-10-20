//! The attach status line (`deckhand_client::attach_status`) in a real PTY: for every agent the line goes on the top
//! row with a rule under it, the agent gets two rows less, and its scrolling, screen clears, scroll-region resets and
//! far cursor moves never reach either.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::Duration;

use deckhand_e2e::harness::{Sandbox, launcher, wait_for};
use deckhand_e2e::ui::{Ui, UiOptions};

/// A stand-in agent (never a real one) that reports its PTY size, then keeps writing: numbered lines that scroll, a
/// full clear every 10, a scroll-region reset plus a move far past its last row every 7.
fn install_noisy_agents(dir: &Path) {
    fs::create_dir_all(dir).expect("agent bin dir");
    let script = r#"#!/bin/sh
for arg in "$@"; do
	case "$arg" in
	--help) echo 'PROMPT --settings --no-daemon resume'; exit 0;;
	--version) echo '1.0.0'; exit 0;;
	esac
done
echo 'fake agent ready'
i=0
while :; do
	i=$((i+1))
	printf 'tick %s size %s\r\n' "$i" "$(stty size)"
	if [ $((i % 10)) = 0 ]; then printf '\033[2J\033[H'; fi
	if [ $((i % 7)) = 0 ]; then printf '\033[r\033[999;1Hlow %s\r\n' "$i"; fi
	sleep 0.05
done
"#;
    for agent in ["claude", "pi", "codex"] {
        let file = dir.join(agent);
        fs::write(&file, script).expect("write agent");
        fs::set_permissions(&file, fs::Permissions::from_mode(0o755)).expect("chmod agent");
    }
}

fn first_row(frame: &str) -> &str {
    frame.split('\n').next().unwrap_or("")
}

#[test]
fn the_status_line_stays_on_the_top_row_for_every_agent() {
    let sandbox = Sandbox::new("attach-status");
    let home = sandbox.dev_home();
    let bin = home.join("bin");
    install_noisy_agents(&bin);
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap_or_default());
    let launcher_script = launcher().display().to_string();
    let mut ui = Ui::spawn(
        &sandbox,
        UiOptions {
            program: Path::new("/bin/sh"),
            args: &[launcher_script.as_str(), "--sandbox"],
            cwd: &home,
            env: &[("PATH", &path)],
            dev: true,
        },
    );
    ui.screen("DEV (isolated)");
    ui.screen("● ready");
    // The agent picker lists Claude, Pi, Codex: `j` moves down (one key at a time, or they arrive as one chunk).
    let options = ["✶ Claude", "π Pi", "◇ Codex"];
    for (index, agent) in ["claude", "pi", "codex"].into_iter().enumerate() {
        let name = format!("noisy-{agent}");
        ui.press("n");
        ui.screen("Choose an agent");
        for option in &options[1..=index] {
            ui.press("j");
            ui.screen(&format!("❯ {option}"));
        }
        ui.press("\r");
        ui.screen("Workspace: new worktree");
        ui.press(&name);
        ui.screen(&format!("Name: {name}"));
        ui.press("\r");
        ui.screen("tick");
        wait_for(
            || ui.screen_text(),
            |frame| frame.contains("q quit") && !frame.contains("Working…"),
            Duration::from_secs(10),
        );
        ui.press("\r");

        // Attached (every agent attach is full screen, so the line goes on top): the agent sees 34 of the 36 rows,
        // and the line and its rule hold the first two through clears and resets.
        ui.screen_within("size 34 130", Duration::from_secs(10));
        for _ in 0..12 {
            std::thread::sleep(Duration::from_millis(250));
            let frame = ui.screen_text();
            let line = first_row(&frame);
            let rule = frame.split('\n').nth(1).unwrap_or("");
            assert!(
                rule.starts_with('─') && rule.trim_end().chars().all(|c| c == '─'),
                "{agent}: rule missing under the status line:\n{frame}"
            );
            assert!(
                line.contains("deckhand") && line.contains(&name) && line.contains("Ctrl+] detach"),
                "{agent}: status line missing from the top row:\n{frame}"
            );
            assert!(
                !line.contains("tick") && !line.contains("low"),
                "{agent}: agent output on the status row:\n{frame}"
            );
        }
        let frame = ui.screen_within("low ", Duration::from_secs(5));
        assert!(frame.contains("deckhand"), "{agent}");

        // Detach: back in the UI, the line gone.
        ui.press("\x1d");
        let frame = ui.screen_within("q quit", Duration::from_secs(10));
        assert!(
            !first_row(&frame).contains("Ctrl+] detach"),
            "{agent}: status line left behind:\n{frame}"
        );
    }
}
