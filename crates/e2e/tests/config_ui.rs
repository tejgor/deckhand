//! Port of tests/configUi.test.ts.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};

use deckhand_e2e::harness::{Sandbox, launcher, wait_for};
use deckhand_e2e::ui::{UI_WAIT, Ui, UiOptions};

/// A stand-in agent (never a real one): stays alive and prints a marker.
fn fake_agent_script(ready: &str) -> String {
    format!(
        "#!/bin/sh\ncase \" $* \" in\n*\" --help \"*) echo 'PROMPT --settings --no-daemon resume'; exit 0;;\n*\" --version \"*) echo '1.0.0'; exit 0;;\nesac\n{ready}\nexec sleep 100000\n"
    )
}

/// claude, pi and codex stand-ins in `<root>/agents`; returns that directory.
fn install_fakes(sandbox: &Sandbox, ready: &str) -> PathBuf {
    let bin = sandbox.root().join("agents");
    fs::create_dir_all(&bin).expect("agents dir");
    for agent in ["claude", "pi", "codex"] {
        let file = bin.join(agent);
        fs::write(&file, fake_agent_script(ready)).expect("write fake agent");
        fs::set_permissions(&file, fs::Permissions::from_mode(0o755)).expect("chmod");
    }
    bin
}

/// The isolated dev launcher (`--sandbox`) in a PTY, with the fakes first on PATH.
fn start_ui(sandbox: &Sandbox, bin: &Path) -> Ui {
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap_or_default());
    // The launcher is not marked executable: run it through sh, as dev_launcher.rs does.
    let launcher = launcher();
    let launcher = launcher.to_string_lossy();
    Ui::spawn(
        sandbox,
        UiOptions {
            program: Path::new("/bin/sh"),
            args: &[&launcher, "--sandbox"],
            cwd: &sandbox.dev_home(),
            env: &[("PATH", &path)],
            dev: true,
        },
    )
}

fn read_json(home: &Path, name: &str) -> Value {
    fs::read_to_string(home.join(name))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_else(|| json!({}))
}

fn trusted_count(config: &Value) -> usize {
    config
        .get("trustedProjects")
        .and_then(Value::as_object)
        .map_or(0, |trusted| trusted.len())
}

/// The first repository's record in ui-state.json.
fn preferences(home: &Path) -> Option<Value> {
    read_json(home, "ui-state.json")
        .as_object()
        .and_then(|state| state.values().next().cloned())
}

fn preference(home: &Path, key: &str) -> Option<String> {
    preferences(home).and_then(|state| state.get(key).and_then(Value::as_str).map(str::to_string))
}

// C → Settings opens on the This repo column; e opens the selected column's raw JSON (← selects Global).
fn open_settings(ui: &mut Ui) {
    ui.press("C");
    ui.screen("Settings · sandbox");
    ui.screen("Global (all repos)");
}

fn open_config(ui: &mut Ui) {
    open_settings(ui);
    ui.press("E");
    ui.screen("Repository config");
}

fn replace(ui: &mut Ui, text: &str, marker: &str) {
    ui.press("\x01");
    ui.screen("all selected");
    ui.press(text);
    ui.screen(marker);
}

// ported from tests/configUi.test.ts: isolated terminal UI asks for trust only when repository config is about to run, edits the raw JSON (C → e) with raw terminal keys, persists search/filter and quits leaving the dev daemon alive
#[test]
fn isolated_terminal_ui_trust_raw_json_editing_filters_and_quit() {
    let sandbox = Sandbox::new("cfg");
    let bin = install_fakes(&sandbox, "echo 'fake agent ready'");
    let mut ui = start_ui(&sandbox, &bin);
    let home = sandbox.dev_home();
    let file = home.join("sandbox").join("deckhand.json");
    ui.screen("DEV (isolated)");
    ui.screen("● ready");

    // The sandbox's deckhand.json (defaultWorkspace "new") is untrusted. Trust gates running, not choosing: n opens the
    // picker at once with the repository's workspace preselected, and only creating a worktree that would run its
    // setupCommand reviews it (Esc returns to the form, s creates with global settings only).
    let mut repo_config: Value = serde_json::from_str(&fs::read_to_string(&file).expect("sandbox deckhand.json"))
        .expect("sandbox deckhand.json is JSON");
    repo_config["setupCommand"] = json!("touch repo-setup-ran");
    fs::write(&file, repo_config.to_string()).expect("write repo config");
    ui.press("n");
    ui.screen("Choose an agent");
    ui.press("\r");
    ui.screen("Workspace: new worktree");
    ui.press("wt1");
    ui.screen("Name: wt1");
    ui.press("\r");
    ui.screen("Review repository configuration");
    ui.screen("setupCommand (touch repo-setup-ran)");
    ui.press("\x1b");
    ui.screen("Workspace: new worktree");
    ui.press("\r");
    ui.screen("s create without the repo's settings");
    ui.press("s");
    ui.screen("fake agent ready");
    assert_eq!(trusted_count(&read_json(&home, "config.json")), 0);
    assert!(home.join("worktrees").join("wt1").exists());
    assert!(!home.join("worktrees").join("wt1").join("repo-setup-ran").exists());
    // r lists Dev, then the untrusted repository action, marked; choosing it reviews first, Esc returns to the list.
    ui.press("r");
    ui.screen("· needs trust");
    ui.press("j");
    ui.screen("Selected command");
    ui.press("\r");
    ui.screen("About to run the repository action test: npm test");
    ui.screen("s cancel");
    ui.press("\x1b");
    ui.screen("· needs trust");
    ui.press("\x1b");
    ui.screen("C settings");
    // T in Settings trusts it explicitly; n still opens the picker directly.
    ui.press("C");
    ui.screen("Settings ·");
    ui.press("T");
    ui.screen("Review repository configuration");
    ui.press("\r");
    ui.screen("trusted ✓");
    ui.press("\x1b");
    ui.screen("C settings");
    wait_for(
        || read_json(&home, "config.json"),
        |config| trusted_count(config) == 1,
        UI_WAIT,
    );
    ui.press("n");
    ui.screen("Choose an agent");
    ui.press("\r");
    ui.screen("Workspace: new worktree");
    ui.press("\x1b");
    ui.screen("Choose an agent");
    ui.press("\x1b");
    ui.screen("C settings");

    // Global defaults are stored in config.json under "defaults", keeping the trust entry.
    open_settings(&mut ui);
    ui.press("\x1b[D");
    ui.screen("Built-in default");
    ui.press("E");
    ui.screen("config.json \"defaults\"");
    ui.press("\x01");
    ui.screen("all selected");
    ui.press("{\"devCommand\":\"printf global\"}");
    ui.screen("printf global");
    ui.press("\x13");
    ui.screen("Saved global defaults");
    ui.screen("Settings · sandbox");
    ui.press("\x1b");
    ui.screen("C settings");
    let config = read_json(&home, "config.json");
    assert_eq!(config["defaults"], json!({"devCommand": "printf global"}));
    assert_eq!(trusted_count(&config), 1);

    // Raw key decoding (useTerminalInput): DEL and Kitty Backspace delete left, forward Delete deletes right, key releases are ignored.
    open_config(&mut ui);
    let direction = "{\"devCommand\":\"abcd\"}";
    replace(&mut ui, direction, "abcd");
    let steps = direction.find("abcd").expect("abcd") + 2;
    ui.press(&format!("\x1b[H{}", "\x1b[C".repeat(steps)));
    ui.screen(direction);
    ui.press("\x7f");
    ui.screen("{\"devCommand\":\"acd\"}");
    ui.press("\x1b[127;1u");
    ui.screen("{\"devCommand\":\"cd\"}");
    ui.press("\x1b[3~");
    ui.screen("{\"devCommand\":\"d\"}");
    ui.write("\x1b[127;1:3u");
    ui.press("\x13");
    ui.screen("Saved deckhand.json · still trusted");
    ui.screen("Settings · sandbox");
    assert_eq!(
        fs::read_to_string(&file).expect("deckhand.json"),
        "{\"devCommand\":\"d\"}"
    );
    ui.press("\x1b");
    ui.screen("C settings");

    // A dirty draft asks before discarding; the first Ctrl+C behaves like Esc here instead of quitting.
    open_config(&mut ui);
    replace(&mut ui, "{\"defaultAgent\":\"pi\"}", "defaultAgent");
    ui.press("\x03");
    ui.screen("Discard unsaved configuration?");
    ui.press("\x1b");
    ui.screen("Repository config");
    ui.press("\x1b");
    ui.screen("Discard unsaved configuration?");
    ui.press("\r");
    ui.screen("Settings · sandbox");
    ui.press("\x1b");
    ui.screen("C settings");
    assert_eq!(
        fs::read_to_string(&file).expect("deckhand.json"),
        "{\"devCommand\":\"d\"}"
    );

    let until = |key: &str, value: &str| {
        wait_for(
            || preference(&home, key),
            |state| state.as_deref() == Some(value),
            UI_WAIT,
        );
    };
    // f opens the filter menu and a key picks one; `fA` typed fast arrives as one chunk.
    ui.press("fA");
    until("sessionFilter", "archived");
    // j shows the next filter at once; Esc restores the one the menu was opened on; f f goes back to active.
    ui.press("f");
    ui.screen("f back to active");
    ui.press("j");
    until("sessionFilter", "all");
    ui.press("\x1b");
    until("sessionFilter", "archived");
    ui.press("f");
    ui.screen("f back to active");
    ui.press("f");
    until("sessionFilter", "active");
    ui.press("f");
    ui.screen("f back to active");
    ui.press("A");
    until("sessionFilter", "archived");
    ui.press("/");
    ui.screen("Search: /");
    ui.press("marker");
    until("sessionQuery", "marker");
    ui.press("\r");
    ui.screen("C settings");
    // Ctrl+C again at the discard prompt quits (so the editor can never trap the UI).
    open_config(&mut ui);
    replace(&mut ui, "{\"defaultAgent\":\"pi\"}", "defaultAgent");
    ui.press("\x03");
    ui.screen("Discard unsaved configuration?");
    ui.press("\x03");
    let status = ui.ended(UI_WAIT).expect("the UI quits");
    assert_eq!(status.code(), Some(0));
    assert_eq!(
        fs::read_to_string(&file).expect("deckhand.json"),
        "{\"devCommand\":\"d\"}"
    );
    // Not Sandbox::exec_launcher: scripts/deckhand-dev.sh is not executable, so run it through sh.
    let mut status_command = sandbox.command("/bin/sh");
    status_command.arg(launcher()).arg("status");
    sandbox.dev_env(&mut status_command);
    let output = sandbox.exec(status_command, Duration::from_secs(30));
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("Isolated dev daemon running"),
        "status: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
