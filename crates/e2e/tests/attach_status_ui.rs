//! The attach status line (`deckhand_client::attach_status`) in a real PTY: for every agent, the Terminal pane and
//! lazygit, the rule and the line hold the bottom two rows. The program gets two rows less, and its scrolling, screen
//! clears, scroll-region resets and far cursor moves never reach either.

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

/// A stand-in lazygit (never the real one): the alternate screen, then its PTY size on its first row, over and over.
fn install_fake_lazygit(dir: &Path) {
    let file = dir.join("lazygit");
    let script = "#!/bin/sh\nprintf '\\033[?1049h\\033[2J'\nwhile :; do\n\tprintf '\\033[Hgit size %s' \"$(stty size)\"\n\tsleep 0.05\ndone\n";
    fs::write(&file, script).expect("write lazygit");
    fs::set_permissions(&file, fs::Permissions::from_mode(0o755)).expect("chmod lazygit");
}

fn first_row(frame: &str) -> &str {
    frame.split('\n').next().unwrap_or("")
}

/// The frame's rows from the bottom (`0`: the last row).
fn row_from_bottom(frame: &str, index: usize) -> &str {
    frame.trim_end_matches('\n').split('\n').rev().nth(index).unwrap_or("")
}

fn is_rule(row: &str) -> bool {
    row.starts_with('─') && row.trim_end().chars().all(|c| c == '─')
}

#[test]
fn the_status_line_holds_its_rows_for_every_agent_and_pane() {
    let sandbox = Sandbox::new("attach-status");
    let home = sandbox.dev_home();
    let bin = home.join("bin");
    install_noisy_agents(&bin);
    install_fake_lazygit(&bin);
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

        // Attached: the agent sees 34 of the 36 rows, and the rule and the line hold the last two through clears and
        // resets (the agent's moves far down land on its own last row).
        ui.screen_within("size 34 130", Duration::from_secs(10));
        for _ in 0..12 {
            std::thread::sleep(Duration::from_millis(250));
            let frame = ui.screen_text();
            let (rule, line) = (row_from_bottom(&frame, 1), row_from_bottom(&frame, 0));
            assert!(is_rule(rule), "{agent}: rule missing over the status line:\n{frame}");
            assert!(
                line.contains("deckhand") && line.contains(&name) && line.contains("Ctrl+] detach"),
                "{agent}: status line missing from the bottom row:\n{frame}"
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
            !frame.contains("│ Ctrl+] detach"),
            "{agent}: status line left behind:\n{frame}"
        );
    }

    // The Terminal pane (normal screen): the shell keeps the top rows (and its scrollback) and sees 34 of them;
    // scrolling never reaches the rule or the line.
    ui.press("t");
    ui.screen("enter open");
    ui.press("\r");
    let frame = ui.screen_within("· terminal", Duration::from_secs(10));
    assert!(
        row_from_bottom(&frame, 0).contains("deckhand") && row_from_bottom(&frame, 0).contains("noisy-codex"),
        "terminal: status line missing from the bottom row:\n{frame}"
    );
    ui.press("stty size\r");
    ui.screen_within("34 130", Duration::from_secs(10));
    ui.press("seq 1 200\r");
    let frame = ui.screen_within("200", Duration::from_secs(10));
    assert!(
        is_rule(row_from_bottom(&frame, 1)),
        "terminal: rule missing over the line:\n{frame}"
    );
    assert!(
        row_from_bottom(&frame, 0).contains("· terminal") && !row_from_bottom(&frame, 0).contains("200"),
        "terminal: shell output on the status row:\n{frame}"
    );
    ui.press("\x1d");
    let frame = ui.screen_within("q quit", Duration::from_secs(10));
    assert!(
        !frame.contains("· terminal │"),
        "terminal: status line left behind:\n{frame}"
    );

    // The Git pane (lazygit, full screen): the same two rows.
    ui.press("g");
    ui.screen("enter lazygit");
    ui.press("\r");
    let frame = ui.screen_within("git size 34 130", Duration::from_secs(10));
    assert!(
        first_row(&frame).starts_with("git size"),
        "git: lazygit off its first row:\n{frame}"
    );
    assert!(
        row_from_bottom(&frame, 0).contains("deckhand") && row_from_bottom(&frame, 0).contains("· git"),
        "git: status line missing from the bottom row:\n{frame}"
    );
    assert!(is_rule(row_from_bottom(&frame, 1)), "git: rule missing:\n{frame}");
    ui.press("\x1d");
    ui.screen_within("q quit", Duration::from_secs(10));
}
